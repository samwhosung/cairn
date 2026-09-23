//! Reads World of Warcraft 1.12.1 DBC tables, given a schema for their untyped columns.

mod csv;
mod error;
mod parser;
mod records;
mod schema;

pub use csv::export_to_csv;
pub use error::Error;
pub use parser::{DbcParser, Header};
pub use records::{Record, RecordSet, StringRef, Value};
pub use schema::{FieldType, Schema, SchemaField};
