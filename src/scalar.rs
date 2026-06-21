use crate::Result;

pub trait QueryForgeEncode {
    type Storage;

    fn queryforge_encode(self) -> Self::Storage;
}

pub trait QueryForgeDecode: Sized {
    type Storage;

    fn queryforge_decode(value: Self::Storage) -> Result<Self>;
}

macro_rules! identity_scalar {
    ($($ty:ty),* $(,)?) => {
        $(
            impl QueryForgeEncode for $ty {
                type Storage = Self;

                fn queryforge_encode(self) -> Self::Storage {
                    self
                }
            }

            impl QueryForgeDecode for $ty {
                type Storage = Self;

                fn queryforge_decode(value: Self::Storage) -> Result<Self> {
                    Ok(value)
                }
            }
        )*
    };
}

identity_scalar!(bool, i16, i32, i64, f32, f64, String, Vec<u8>);

impl<T> QueryForgeEncode for Option<T>
where
    T: QueryForgeEncode,
{
    type Storage = Option<T::Storage>;

    fn queryforge_encode(self) -> Self::Storage {
        self.map(QueryForgeEncode::queryforge_encode)
    }
}

#[cfg(feature = "uuid-types")]
identity_scalar!(uuid::Uuid);

#[cfg(feature = "serde-json-types")]
identity_scalar!(serde_json::Value);

#[cfg(feature = "decimal-types")]
identity_scalar!(rust_decimal::Decimal);

#[cfg(feature = "chrono-types")]
identity_scalar!(
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
);

#[cfg(feature = "time-types")]
identity_scalar!(
    time::Date,
    time::Time,
    time::PrimitiveDateTime,
    time::OffsetDateTime,
);

#[macro_export]
macro_rules! scalar_newtype {
    ($ty:ty, $storage:ty) => {
        impl $crate::QueryForgeEncode for $ty {
            type Storage = $storage;

            fn queryforge_encode(self) -> Self::Storage {
                self.0
            }
        }

        impl $crate::QueryForgeDecode for $ty {
            type Storage = $storage;

            fn queryforge_decode(value: Self::Storage) -> $crate::Result<Self> {
                Ok(Self(value))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{QueryForgeDecode, QueryForgeEncode};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct AuthorId(i64);

    crate::scalar_newtype!(AuthorId, i64);

    #[test]
    fn scalar_newtype_encodes_and_decodes_storage() {
        assert_eq!(AuthorId(42).queryforge_encode(), 42);
        assert_eq!(AuthorId::queryforge_decode(42).unwrap(), AuthorId(42));
    }
}
