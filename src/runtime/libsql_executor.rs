use std::future::Future;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum LibsqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    Boolean(bool),
}

impl From<i16> for LibsqlValue {
    fn from(value: i16) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<i32> for LibsqlValue {
    fn from(value: i32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<i64> for LibsqlValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<f32> for LibsqlValue {
    fn from(value: f32) -> Self {
        Self::Real(f64::from(value))
    }
}

impl From<f64> for LibsqlValue {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<bool> for LibsqlValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<String> for LibsqlValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for LibsqlValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<Vec<u8>> for LibsqlValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

#[cfg(feature = "uuid-types")]
impl From<uuid::Uuid> for LibsqlValue {
    fn from(value: uuid::Uuid) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "serde-json-types")]
impl From<serde_json::Value> for LibsqlValue {
    fn from(value: serde_json::Value) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "decimal-types")]
impl From<rust_decimal::Decimal> for LibsqlValue {
    fn from(value: rust_decimal::Decimal) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "chrono-types")]
impl From<chrono::NaiveDate> for LibsqlValue {
    fn from(value: chrono::NaiveDate) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "chrono-types")]
impl From<chrono::NaiveTime> for LibsqlValue {
    fn from(value: chrono::NaiveTime) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "chrono-types")]
impl From<chrono::NaiveDateTime> for LibsqlValue {
    fn from(value: chrono::NaiveDateTime) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "chrono-types")]
impl From<chrono::DateTime<chrono::Utc>> for LibsqlValue {
    fn from(value: chrono::DateTime<chrono::Utc>) -> Self {
        Self::Text(value.to_rfc3339())
    }
}

#[cfg(feature = "time-types")]
impl From<time::Date> for LibsqlValue {
    fn from(value: time::Date) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "time-types")]
impl From<time::Time> for LibsqlValue {
    fn from(value: time::Time) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "time-types")]
impl From<time::PrimitiveDateTime> for LibsqlValue {
    fn from(value: time::PrimitiveDateTime) -> Self {
        Self::Text(value.to_string())
    }
}

#[cfg(feature = "time-types")]
impl From<time::OffsetDateTime> for LibsqlValue {
    fn from(value: time::OffsetDateTime) -> Self {
        Self::Text(
            value
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| value.to_string()),
        )
    }
}

impl<T> From<Option<T>> for LibsqlValue
where
    T: Into<LibsqlValue>,
{
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or(Self::Null)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibsqlRow {
    values: Vec<(String, LibsqlValue)>,
}

impl LibsqlRow {
    pub fn new(values: impl IntoIterator<Item = (impl Into<String>, LibsqlValue)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        }
    }

    pub fn try_get<T>(&self, column: &str) -> Result<T>
    where
        T: LibsqlDecode,
    {
        let value = self
            .values
            .iter()
            .find_map(|(name, value)| (name == column).then_some(value))
            .ok_or_else(|| Error::Backend(format!("libSQL row has no column `{column}`")))?;
        T::decode(value)
    }

    pub fn try_get_index<T>(&self, index: usize) -> Result<T>
    where
        T: LibsqlDecode,
    {
        let value = self
            .values
            .get(index)
            .map(|(_, value)| value)
            .ok_or_else(|| {
                Error::Backend(format!("libSQL row has no column at index `{index}`"))
            })?;
        T::decode(value)
    }
}

pub trait LibsqlDecode: Sized {
    fn decode(value: &LibsqlValue) -> Result<Self>;
}

impl LibsqlDecode for bool {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Boolean(value) => Ok(*value),
            LibsqlValue::Integer(value) => Ok(*value != 0),
            other => Err(decode_error("bool", other)),
        }
    }
}

impl LibsqlDecode for i16 {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        i64::decode(value)?
            .try_into()
            .map_err(|_| Error::Backend("libSQL integer does not fit i16".to_string()))
    }
}

impl LibsqlDecode for i32 {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        i64::decode(value)?
            .try_into()
            .map_err(|_| Error::Backend("libSQL integer does not fit i32".to_string()))
    }
}

impl LibsqlDecode for i64 {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Integer(value) => Ok(*value),
            other => Err(decode_error("i64", other)),
        }
    }
}

impl LibsqlDecode for f32 {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        Ok(f64::decode(value)? as f32)
    }
}

impl LibsqlDecode for f64 {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Real(value) => Ok(*value),
            LibsqlValue::Integer(value) => Ok(*value as f64),
            other => Err(decode_error("f64", other)),
        }
    }
}

impl LibsqlDecode for String {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Text(value) => Ok(value.clone()),
            other => Err(decode_error("String", other)),
        }
    }
}

impl LibsqlDecode for Vec<u8> {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Blob(value) => Ok(value.clone()),
            other => Err(decode_error("Vec<u8>", other)),
        }
    }
}

#[cfg(feature = "uuid-types")]
impl LibsqlDecode for uuid::Uuid {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Text(text) => uuid::Uuid::parse_str(text).map_err(|err| {
                Error::Backend(format!(
                    "failed to decode libSQL text value as uuid::Uuid: {err}"
                ))
            }),
            LibsqlValue::Blob(bytes) => uuid::Uuid::from_slice(bytes).map_err(|err| {
                Error::Backend(format!(
                    "failed to decode libSQL blob value as uuid::Uuid: {err}"
                ))
            }),
            other => Err(decode_error("uuid::Uuid", other)),
        }
    }
}

#[cfg(feature = "serde-json-types")]
impl LibsqlDecode for serde_json::Value {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("serde_json::Value", value)?;
        serde_json::from_str(text).map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as serde_json::Value: {err}"
            ))
        })
    }
}

#[cfg(feature = "decimal-types")]
impl LibsqlDecode for rust_decimal::Decimal {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("rust_decimal::Decimal", value)?;
        text.parse::<rust_decimal::Decimal>().map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as rust_decimal::Decimal: {err}"
            ))
        })
    }
}

#[cfg(feature = "chrono-types")]
impl LibsqlDecode for chrono::NaiveDate {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("chrono::NaiveDate", value)?;
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as chrono::NaiveDate: {err}"
            ))
        })
    }
}

#[cfg(feature = "chrono-types")]
impl LibsqlDecode for chrono::NaiveTime {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("chrono::NaiveTime", value)?;
        chrono::NaiveTime::parse_from_str(text, "%H:%M:%S%.f").map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as chrono::NaiveTime: {err}"
            ))
        })
    }
}

#[cfg(feature = "chrono-types")]
impl LibsqlDecode for chrono::NaiveDateTime {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("chrono::NaiveDateTime", value)?;
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f"))
            .map_err(|err| {
                Error::Backend(format!(
                    "failed to decode libSQL value as chrono::NaiveDateTime: {err}"
                ))
            })
    }
}

#[cfg(feature = "chrono-types")]
impl LibsqlDecode for chrono::DateTime<chrono::Utc> {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("chrono::DateTime<chrono::Utc>", value)?;
        chrono::DateTime::parse_from_rfc3339(text)
            .map(|value| value.with_timezone(&chrono::Utc))
            .map_err(|err| {
                Error::Backend(format!(
                    "failed to decode libSQL value as chrono::DateTime<chrono::Utc>: {err}"
                ))
            })
    }
}

#[cfg(feature = "time-types")]
impl LibsqlDecode for time::Date {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("time::Date", value)?;
        time::Date::parse(
            text,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as time::Date: {err}"
            ))
        })
    }
}

#[cfg(feature = "time-types")]
impl LibsqlDecode for time::Time {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("time::Time", value)?;
        time::Time::parse(
            text,
            time::macros::format_description!("[hour]:[minute]:[second].[subsecond]"),
        )
        .or_else(|_| {
            time::Time::parse(
                text,
                time::macros::format_description!("[hour]:[minute]:[second]"),
            )
        })
        .map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as time::Time: {err}"
            ))
        })
    }
}

#[cfg(feature = "time-types")]
impl LibsqlDecode for time::PrimitiveDateTime {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("time::PrimitiveDateTime", value)?;
        time::PrimitiveDateTime::parse(
            text,
            time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond]"
            ),
        )
        .or_else(|_| {
            time::PrimitiveDateTime::parse(
                text,
                time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
            )
        })
        .map_err(|err| {
            Error::Backend(format!(
                "failed to decode libSQL value as time::PrimitiveDateTime: {err}"
            ))
        })
    }
}

#[cfg(feature = "time-types")]
impl LibsqlDecode for time::OffsetDateTime {
    fn decode(value: &LibsqlValue) -> Result<Self> {
        let text = decode_text("time::OffsetDateTime", value)?;
        time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).map_err(
            |err| {
                Error::Backend(format!(
                    "failed to decode libSQL value as time::OffsetDateTime: {err}"
                ))
            },
        )
    }
}

impl<T> LibsqlDecode for Option<T>
where
    T: LibsqlDecode,
{
    fn decode(value: &LibsqlValue) -> Result<Self> {
        match value {
            LibsqlValue::Null => Ok(None),
            other => T::decode(other).map(Some),
        }
    }
}

fn decode_error(expected: &str, actual: &LibsqlValue) -> Error {
    Error::Backend(format!(
        "failed to decode libSQL value as {expected}: {actual:?}"
    ))
}

#[cfg(any(
    feature = "uuid-types",
    feature = "serde-json-types",
    feature = "time-types",
    feature = "chrono-types",
    feature = "decimal-types"
))]
#[cfg(any(
    feature = "serde-json-types",
    feature = "decimal-types",
    feature = "chrono-types",
    feature = "time-types"
))]
fn decode_text<'a>(expected: &str, value: &'a LibsqlValue) -> Result<&'a str> {
    match value {
        LibsqlValue::Text(value) => Ok(value),
        other => Err(decode_error(expected, other)),
    }
}

#[cfg(feature = "libsql-runtime")]
impl From<LibsqlValue> for libsql::Value {
    fn from(value: LibsqlValue) -> Self {
        match value {
            LibsqlValue::Null => Self::Null,
            LibsqlValue::Integer(value) => Self::Integer(value),
            LibsqlValue::Real(value) => Self::Real(value),
            LibsqlValue::Text(value) => Self::Text(value),
            LibsqlValue::Blob(value) => Self::Blob(value),
            LibsqlValue::Boolean(value) => Self::Integer(i64::from(value)),
        }
    }
}

pub trait LibsqlExecutor {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<u64>> + Send + 'a;

    fn query_one<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<LibsqlRow>> + Send + 'a;

    fn query_optional<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<Option<LibsqlRow>>> + Send + 'a;

    fn query_many<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<Vec<LibsqlRow>>> + Send + 'a;
}

#[cfg(feature = "libsql-runtime")]
impl LibsqlExecutor for libsql::Connection {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<u64>> + Send + 'a {
        let params = to_libsql_params(params);
        async move { execute_on_libsql_connection(self, sql, params).await }
    }

    fn query_one<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<LibsqlRow>> + Send + 'a {
        let params = to_libsql_params(params);
        async move { query_one_on_libsql_connection(self, sql, params).await }
    }

    fn query_optional<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<Option<LibsqlRow>>> + Send + 'a {
        let params = to_libsql_params(params);
        async move { query_optional_on_libsql_connection(self, sql, params).await }
    }

    fn query_many<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<Vec<LibsqlRow>>> + Send + 'a {
        let params = to_libsql_params(params);
        async move { query_many_on_libsql_connection(self, sql, params).await }
    }
}

#[cfg(feature = "libsql-runtime")]
impl LibsqlExecutor for libsql::Transaction {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<u64>> + Send + 'a {
        let params = to_libsql_params(params);
        async move {
            let conn: &libsql::Connection = self;
            execute_on_libsql_connection(conn, sql, params).await
        }
    }

    fn query_one<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<LibsqlRow>> + Send + 'a {
        let params = to_libsql_params(params);
        async move {
            let conn: &libsql::Connection = self;
            query_one_on_libsql_connection(conn, sql, params).await
        }
    }

    fn query_optional<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<Option<LibsqlRow>>> + Send + 'a {
        let params = to_libsql_params(params);
        async move {
            let conn: &libsql::Connection = self;
            query_optional_on_libsql_connection(conn, sql, params).await
        }
    }

    fn query_many<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [LibsqlValue],
    ) -> impl Future<Output = Result<Vec<LibsqlRow>>> + Send + 'a {
        let params = to_libsql_params(params);
        async move {
            let conn: &libsql::Connection = self;
            query_many_on_libsql_connection(conn, sql, params).await
        }
    }
}

#[cfg(feature = "libsql-runtime")]
fn to_libsql_params(params: &[LibsqlValue]) -> Vec<libsql::Value> {
    params.iter().cloned().map(Into::into).collect()
}

#[cfg(feature = "libsql-runtime")]
async fn execute_on_libsql_connection(
    conn: &libsql::Connection,
    sql: &str,
    params: Vec<libsql::Value>,
) -> Result<u64> {
    libsql::Connection::execute(conn, sql, params)
        .await
        .map_err(map_libsql_error)
}

#[cfg(feature = "libsql-runtime")]
async fn query_one_on_libsql_connection(
    conn: &libsql::Connection,
    sql: &str,
    params: Vec<libsql::Value>,
) -> Result<LibsqlRow> {
    let mut rows = libsql::Connection::query(conn, sql, params)
        .await
        .map_err(map_libsql_error)?;
    let row = rows
        .next()
        .await
        .map_err(map_libsql_error)?
        .ok_or_else(|| Error::Backend("libSQL query returned no rows".to_string()))?;
    from_libsql_row(row)
}

#[cfg(feature = "libsql-runtime")]
async fn query_optional_on_libsql_connection(
    conn: &libsql::Connection,
    sql: &str,
    params: Vec<libsql::Value>,
) -> Result<Option<LibsqlRow>> {
    let mut rows = libsql::Connection::query(conn, sql, params)
        .await
        .map_err(map_libsql_error)?;
    rows.next()
        .await
        .map_err(map_libsql_error)?
        .map(from_libsql_row)
        .transpose()
}

#[cfg(feature = "libsql-runtime")]
async fn query_many_on_libsql_connection(
    conn: &libsql::Connection,
    sql: &str,
    params: Vec<libsql::Value>,
) -> Result<Vec<LibsqlRow>> {
    let mut rows = libsql::Connection::query(conn, sql, params)
        .await
        .map_err(map_libsql_error)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_error)? {
        out.push(from_libsql_row(row)?);
    }
    Ok(out)
}

#[cfg(feature = "libsql-runtime")]
fn from_libsql_row(row: libsql::Row) -> Result<LibsqlRow> {
    let column_count = row.column_count();
    let mut values = Vec::with_capacity(column_count as usize);
    for idx in 0..column_count {
        let name = row
            .column_name(idx)
            .ok_or_else(|| Error::Backend(format!("libSQL row column {idx} has no name")))?
            .to_string();
        let value = row.get_value(idx).map_err(map_libsql_error)?;
        values.push((name, from_libsql_value(value)));
    }
    Ok(LibsqlRow::new(values))
}

#[cfg(feature = "libsql-runtime")]
fn from_libsql_value(value: libsql::Value) -> LibsqlValue {
    match value {
        libsql::Value::Null => LibsqlValue::Null,
        libsql::Value::Integer(value) => LibsqlValue::Integer(value),
        libsql::Value::Real(value) => LibsqlValue::Real(value),
        libsql::Value::Text(value) => LibsqlValue::Text(value),
        libsql::Value::Blob(value) => LibsqlValue::Blob(value),
    }
}

#[cfg(feature = "libsql-runtime")]
fn map_libsql_error(error: libsql::Error) -> Error {
    Error::Backend(format!("libSQL error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn decodes_required_and_optional_values() {
        let row = LibsqlRow::new([
            ("id", LibsqlValue::Integer(42)),
            ("email", LibsqlValue::Text("a@example.com".to_string())),
            ("deleted_at", LibsqlValue::Null),
        ]);

        assert_eq!(row.try_get::<i64>("id").unwrap(), 42);
        assert_eq!(
            row.try_get::<String>("email").unwrap(),
            "a@example.com".to_string()
        );
        assert_eq!(row.try_get::<Option<String>>("deleted_at").unwrap(), None);
    }

    #[test]
    fn preserves_duplicate_column_names_for_index_decoding() {
        let row = LibsqlRow::new([
            ("id", LibsqlValue::Integer(1)),
            ("id", LibsqlValue::Integer(2)),
        ]);

        assert_eq!(row.try_get::<i64>("id").unwrap(), 1);
        assert_eq!(row.try_get_index::<i64>(0).unwrap(), 1);
        assert_eq!(row.try_get_index::<i64>(1).unwrap(), 2);
    }

    #[test]
    fn converts_params_into_runtime_values() {
        assert_eq!(LibsqlValue::from(1_i64), LibsqlValue::Integer(1));
        assert_eq!(
            LibsqlValue::from(Some("hello")),
            LibsqlValue::Text("hello".to_string())
        );
        assert_eq!(LibsqlValue::from(None::<i64>), LibsqlValue::Null);
    }

    #[cfg(all(
        feature = "uuid-types",
        feature = "serde-json-types",
        feature = "time-types",
        feature = "chrono-types",
        feature = "decimal-types"
    ))]
    #[test]
    fn external_type_adapters_round_trip_text_values() {
        let id = uuid::Uuid::nil();
        let payload = serde_json::json!({ "ok": true });
        let amount = rust_decimal::Decimal::new(1234, 2);
        let time_value = time::OffsetDateTime::UNIX_EPOCH;
        let chrono_value = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;

        let row = LibsqlRow::new([
            ("id", LibsqlValue::from(id)),
            ("id_blob", LibsqlValue::Blob(id.as_bytes().to_vec())),
            ("payload", LibsqlValue::from(payload.clone())),
            ("amount", LibsqlValue::from(amount)),
            ("time_value", LibsqlValue::from(time_value)),
            ("chrono_value", LibsqlValue::from(chrono_value)),
        ]);

        assert_eq!(row.try_get::<uuid::Uuid>("id").unwrap(), id);
        assert_eq!(row.try_get::<uuid::Uuid>("id_blob").unwrap(), id);
        assert_eq!(
            row.try_get::<serde_json::Value>("payload").unwrap(),
            payload
        );
        assert_eq!(
            row.try_get::<rust_decimal::Decimal>("amount").unwrap(),
            amount
        );
        assert_eq!(
            row.try_get::<time::OffsetDateTime>("time_value").unwrap(),
            time_value
        );
        assert_eq!(
            row.try_get::<chrono::DateTime<chrono::Utc>>("chrono_value")
                .unwrap(),
            chrono_value
        );
    }

    #[test]
    fn fake_executor_exercises_trait_contract() {
        let executor = FakeExecutor {
            rows: vec![LibsqlRow::new([
                ("id", LibsqlValue::Integer(7)),
                ("email", LibsqlValue::Text("a@example.com".to_string())),
            ])],
            calls: Mutex::new(Vec::new()),
        };

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let params = vec![LibsqlValue::Integer(7)];
            let row = executor
                .query_one("SELECT id, email FROM users WHERE id = ?1", &params)
                .await
                .unwrap();
            assert_eq!(row.try_get::<i64>("id").unwrap(), 7);
            assert_eq!(
                executor
                    .execute("UPDATE users SET email = ?1", &[])
                    .await
                    .unwrap(),
                1
            );
        });

        let calls = executor.calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[
                "query_one:SELECT id, email FROM users WHERE id = ?1",
                "execute:UPDATE users SET email = ?1"
            ]
        );
    }

    #[cfg(feature = "libsql-runtime")]
    #[test]
    fn libsql_connection_and_transaction_adapters_execute_queries() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let db = libsql::Builder::new_local(":memory:")
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();

            libsql::Connection::execute(
                &conn,
                "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
                (),
            )
            .await
            .unwrap();

            let inserted = LibsqlExecutor::execute(
                &conn,
                "INSERT INTO users (id, email) VALUES (?1, ?2)",
                &[
                    LibsqlValue::Integer(1),
                    LibsqlValue::Text("a@example.com".to_string()),
                ],
            )
            .await
            .unwrap();
            assert_eq!(inserted, 1);

            let row = LibsqlExecutor::query_one(
                &conn,
                "SELECT id, email FROM users WHERE id = ?1",
                &[LibsqlValue::Integer(1)],
            )
            .await
            .unwrap();
            assert_eq!(row.try_get::<i64>("id").unwrap(), 1);
            assert_eq!(
                row.try_get::<String>("email").unwrap(),
                "a@example.com".to_string()
            );

            let tx = conn.transaction().await.unwrap();
            LibsqlExecutor::execute(
                &tx,
                "INSERT INTO users (id, email) VALUES (?1, ?2)",
                &[
                    LibsqlValue::Integer(2),
                    LibsqlValue::Text("b@example.com".to_string()),
                ],
            )
            .await
            .unwrap();
            let tx_row = LibsqlExecutor::query_optional(
                &tx,
                "SELECT email FROM users WHERE id = ?1",
                &[LibsqlValue::Integer(2)],
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                tx_row.try_get::<String>("email").unwrap(),
                "b@example.com".to_string()
            );
            tx.rollback().await.unwrap();

            let rows = LibsqlExecutor::query_many(&conn, "SELECT id FROM users ORDER BY id", &[])
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].try_get::<i64>("id").unwrap(), 1);
        });
    }

    struct FakeExecutor {
        rows: Vec<LibsqlRow>,
        calls: Mutex<Vec<String>>,
    }

    impl LibsqlExecutor for FakeExecutor {
        fn execute<'a>(
            &'a self,
            sql: &'a str,
            _params: &'a [LibsqlValue],
        ) -> impl Future<Output = Result<u64>> + Send + 'a {
            async move {
                self.calls.lock().unwrap().push(format!("execute:{sql}"));
                Ok(1)
            }
        }

        fn query_one<'a>(
            &'a self,
            sql: &'a str,
            _params: &'a [LibsqlValue],
        ) -> impl Future<Output = Result<LibsqlRow>> + Send + 'a {
            async move {
                self.calls.lock().unwrap().push(format!("query_one:{sql}"));
                self.rows
                    .first()
                    .cloned()
                    .ok_or_else(|| Error::Backend("expected one fake row".to_string()))
            }
        }

        fn query_optional<'a>(
            &'a self,
            sql: &'a str,
            _params: &'a [LibsqlValue],
        ) -> impl Future<Output = Result<Option<LibsqlRow>>> + Send + 'a {
            async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("query_optional:{sql}"));
                Ok(self.rows.first().cloned())
            }
        }

        fn query_many<'a>(
            &'a self,
            sql: &'a str,
            _params: &'a [LibsqlValue],
        ) -> impl Future<Output = Result<Vec<LibsqlRow>>> + Send + 'a {
            async move {
                self.calls.lock().unwrap().push(format!("query_many:{sql}"));
                Ok(self.rows.clone())
            }
        }
    }
}
