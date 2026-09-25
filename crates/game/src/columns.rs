use crate::Bytes;

/// A saved field's value as a world's file holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sql {
    Integer,
    Real,
}

impl Sql {
    pub fn name(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Real => "REAL",
        }
    }
}

/// A column of a saved table: its name, what it holds, and whether it may hold nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub sql: Sql,
    pub nullable: bool,
}

/// A value a saved field may hold.
pub trait Column: Bytes {
    const SQL: Sql;
    const NULLABLE: bool = false;

    fn value(&self) -> Value;

    fn from_value(value: Value) -> Option<Self>;
}

/// A column that is never null, so an option of it may be.
pub trait Scalar: Column {}

macro_rules! integers {
    ($($t:ty),*) => {$(
        impl Column for $t {
            const SQL: Sql = Sql::Integer;

            fn value(&self) -> Value {
                Value::Integer(i64::from(*self))
            }

            fn from_value(value: Value) -> Option<Self> {
                match value {
                    Value::Integer(v) => Self::try_from(v).ok(),
                    _ => None,
                }
            }
        }

        impl Scalar for $t {}
    )*};
}

integers!(u8, u16, u32, i8, i16, i32, i64);

impl Column for f64 {
    const SQL: Sql = Sql::Real;

    fn value(&self) -> Value {
        Value::Real(*self)
    }

    fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::Real(v) => Some(v),
            _ => None,
        }
    }
}

impl Scalar for f64 {}

impl Column for f32 {
    const SQL: Sql = Sql::Real;

    fn value(&self) -> Value {
        Value::Real(f64::from(*self))
    }

    fn from_value(value: Value) -> Option<Self> {
        let v = f64::from_value(value)?;
        let narrow = v as f32;
        (f64::from(narrow).to_bits() == v.to_bits()).then_some(narrow)
    }
}

impl Scalar for f32 {}

impl Column for bool {
    const SQL: Sql = Sql::Integer;

    fn value(&self) -> Value {
        Value::Integer(i64::from(*self))
    }

    fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::Integer(0) => Some(false),
            Value::Integer(1) => Some(true),
            _ => None,
        }
    }
}

impl Scalar for bool {}

impl<T: Scalar> Column for Option<T> {
    const SQL: Sql = T::SQL;
    const NULLABLE: bool = true;

    fn value(&self) -> Value {
        self.as_ref().map_or(Value::Null, Column::value)
    }

    fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::Null => Some(None),
            v => T::from_value(v).map(Some),
        }
    }
}

/// What a kind saves, as [`saved!`](crate::saved) declares it: the columns of one table in a
/// world's file.
pub trait Columns: Bytes + PartialEq {
    /// The table and its columns; `None` for a kind that saves nothing.
    const TABLE: Option<(&'static str, &'static [Field])>;

    fn values(&self) -> Vec<Value>;

    /// `None` unless `values` hold one value for each column, each one its field may hold.
    fn from_values(values: &[Value]) -> Option<Self>;

    /// Each column's value in a row saved before the column was.
    fn defaults() -> Vec<Value>;
}

impl Columns for () {
    const TABLE: Option<(&'static str, &'static [Field])> = None;

    fn values(&self) -> Vec<Value> {
        Vec::new()
    }

    fn from_values(values: &[Value]) -> Option<Self> {
        values.is_empty().then_some(())
    }

    fn defaults() -> Vec<Value> {
        Vec::new()
    }
}

/// A saved table whatever its row's type: the server keeps a game's saved fields as bytes and
/// turns them into columns through this.
#[derive(Clone, Copy, Debug)]
pub struct Schema {
    pub name: &'static str,
    pub fields: &'static [Field],
    values: fn(&[u8]) -> Option<Vec<Value>>,
    bytes: fn(&[Value]) -> Option<Vec<u8>>,
    defaults: fn() -> Vec<Value>,
}

impl Schema {
    pub fn of<C: Columns>() -> Option<Self> {
        let (name, fields) = C::TABLE?;
        Some(Self {
            name,
            fields,
            values: |bytes| C::from_bytes(bytes).map(|c| c.values()),
            bytes: |values| C::from_values(values).map(|c| c.to_bytes()),
            defaults: C::defaults,
        })
    }

    /// The columns of a row saved as `bytes`; `None` when they are not one.
    pub fn values(&self, bytes: &[u8]) -> Option<Vec<Value>> {
        (self.values)(bytes)
    }

    /// The bytes of a row whose columns hold `values`; `None` when a field cannot hold its value.
    pub fn bytes(&self, values: &[Value]) -> Option<Vec<u8>> {
        (self.bytes)(values)
    }

    pub fn defaults(&self) -> Vec<Value> {
        (self.defaults)()
    }
}

/// Declares what a kind saves: a struct whose fields are the columns of its table in a world's
/// file, the table named after the struct. A field may give the value rows saved before it
/// existed take, `= value`; its type's default otherwise.
#[macro_export]
macro_rules! saved {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $($(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty $(= $default:expr)?),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        $vis struct $name {
            $($(#[$fmeta])* $fvis $field: $ty),*
        }

        impl ::std::default::Default for $name {
            fn default() -> Self {
                Self {
                    $($field: $crate::saved!(@default $ty $(, $default)?)),*
                }
            }
        }

        impl $crate::Bytes for $name {
            fn put(&self, out: &mut ::std::vec::Vec<u8>) {
                $($crate::Bytes::put(&self.$field, out);)*
            }

            fn take(input: &mut &[u8]) -> ::std::option::Option<Self> {
                ::std::option::Option::Some(Self {
                    $($field: <$ty as $crate::Bytes>::take(input)?),*
                })
            }
        }

        impl $crate::Columns for $name {
            const TABLE: ::std::option::Option<(&'static str, &'static [$crate::Field])> =
                ::std::option::Option::Some((::std::stringify!($name), &[$($crate::Field {
                    name: ::std::stringify!($field),
                    sql: <$ty as $crate::Column>::SQL,
                    nullable: <$ty as $crate::Column>::NULLABLE,
                }),*]));

            fn values(&self) -> ::std::vec::Vec<$crate::Value> {
                ::std::vec![$($crate::Column::value(&self.$field)),*]
            }

            fn from_values(values: &[$crate::Value]) -> ::std::option::Option<Self> {
                let mut each = values.iter().copied();
                let row = Self {
                    $($field: <$ty as $crate::Column>::from_value(each.next()?)?),*
                };
                each.next().is_none().then_some(row)
            }

            fn defaults() -> ::std::vec::Vec<$crate::Value> {
                $crate::Columns::values(&<Self as ::std::default::Default>::default())
            }
        }
    };
    (@default $ty:ty) => {
        <$ty as ::std::default::Default>::default()
    };
    (@default $ty:ty, $default:expr) => {
        $default
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::saved! {
        struct Tally {
            kills: u32,
            deaths: u32 = 7,
            best: Option<f32>,
            alive: bool,
        }
    }

    #[test]
    fn a_saved_row_goes_to_its_columns_and_back_through_bytes() {
        let row = Tally {
            kills: 3,
            deaths: 1,
            best: Some(2.5),
            alive: true,
        };
        let schema = Schema::of::<Tally>().expect("a table");
        assert_eq!(schema.name, "Tally");
        let names: Vec<&str> = schema.fields.iter().map(|f| f.name).collect();
        assert_eq!(names, ["kills", "deaths", "best", "alive"]);
        assert!(schema.fields[2].nullable && schema.fields[2].sql == Sql::Real);
        let values = schema.values(&row.to_bytes()).expect("its columns");
        assert_eq!(
            values,
            [
                Value::Integer(3),
                Value::Integer(1),
                Value::Real(2.5),
                Value::Integer(1)
            ]
        );
        assert_eq!(schema.bytes(&values), Some(row.to_bytes()));
        assert_eq!(
            schema.defaults(),
            [
                Value::Integer(0),
                Value::Integer(7),
                Value::Null,
                Value::Integer(0)
            ]
        );
    }

    #[test]
    fn a_value_its_field_cannot_hold_is_refused() {
        let schema = Schema::of::<Tally>().expect("a table");
        let row = |kills, best| [kills, Value::Integer(0), best, Value::Integer(0)];
        assert!(schema.bytes(&row(Value::Integer(4), Value::Null)).is_some());
        for bad in [
            row(Value::Integer(-1), Value::Null),
            row(Value::Integer(1 << 40), Value::Null),
            row(Value::Real(4.0), Value::Null),
            row(Value::Integer(4), Value::Real(0.1)),
            row(Value::Null, Value::Null),
        ] {
            assert_eq!(schema.bytes(&bad), None, "{bad:?}");
        }
        assert_eq!(
            schema.bytes(&row(Value::Integer(4), Value::Null)[..3]),
            None
        );
        assert!(Schema::of::<()>().is_none());
    }
}
