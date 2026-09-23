use std::borrow::Cow;

use crate::error::{Error, Result};

/// An offset into a record set's string block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringRef(pub u32);

/// One field, read as its schema type says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    UInt32(u32),
    Int32(i32),
    Float32(f32),
    StringRef(StringRef),
}

/// One row: a value per column, arrays expanded.
#[derive(Debug, Clone)]
pub struct Record {
    pub(crate) values: Vec<Value>,
}

impl Record {
    /// The value in column `i`, where an array of `count` takes `count` columns.
    pub fn get_value(&self, i: usize) -> Option<&Value> {
        self.values.get(i)
    }
}

/// A decoded DBC: its rows and the string block they point into.
pub struct RecordSet {
    pub(crate) field_names: Vec<String>,
    pub(crate) records: Vec<Record>,
    pub(crate) strings: Vec<u8>,
}

impl RecordSet {
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// The string at `r`, up to its NUL or the end of the block, with invalid UTF-8 replaced. An
    /// offset equal to the block's length is the empty string.
    pub fn get_string(&self, r: StringRef) -> Result<Cow<'_, str>> {
        let rest = self
            .strings
            .get(r.0 as usize..)
            .ok_or(Error::BadStringRef(r.0))?;
        let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Ok(String::from_utf8_lossy(&rest[..len]))
    }
}
