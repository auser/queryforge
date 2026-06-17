pub mod generated {
    pub mod sqlx_postgres {
        include!(concat!(env!("OUT_DIR"), "/sqlx_postgres/mod.rs"));
    }

    pub mod tokio_postgres {
        include!(concat!(env!("OUT_DIR"), "/tokio_postgres/mod.rs"));
    }

    pub mod libsql_native {
        include!(concat!(env!("OUT_DIR"), "/libsql_native/mod.rs"));
    }
}

#[cfg(test)]
mod tests {
    use super::generated;

    #[test]
    fn generated_external_type_rows_are_constructible() {
        let id = uuid::Uuid::nil();
        let payload = serde_json::json!({ "ok": true });
        let happened_at = time::OffsetDateTime::UNIX_EPOCH;
        let amount = rust_decimal::Decimal::new(1234, 2);

        let sqlx_row = generated::sqlx_postgres::profiles::GetExternalValuesRow {
            id,
            payload: payload.clone(),
            happened_at,
            amount: Some(amount),
        };
        assert_eq!(sqlx_row.id, id);

        let native_row = generated::libsql_native::profiles::GetExternalValuesRow {
            id,
            payload,
            happened_at,
            amount: Some(amount),
        };
        assert_eq!(native_row.amount, Some(amount));
    }

    #[test]
    fn generated_chrono_rows_are_constructible() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();
        let at_time = chrono::NaiveTime::from_hms_opt(12, 30, 0).unwrap();
        let happened_at = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;

        let sqlx_row = generated::sqlx_postgres::profiles::GetChronoValuesRow {
            day,
            at_time,
            happened_at,
        };
        assert_eq!(sqlx_row.day, day);

        let tokio_row = generated::tokio_postgres::profiles::GetTokioSupportedValuesRow {
            payload: serde_json::json!({ "tokio": true }),
            happened_at,
        };
        assert_eq!(tokio_row.happened_at, happened_at);

        let native_row = generated::libsql_native::profiles::GetChronoValuesRow {
            day,
            at_time,
            happened_at,
        };
        assert_eq!(native_row.happened_at, happened_at);
    }
}
