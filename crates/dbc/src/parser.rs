use std::io::Cursor;

use wowfile::{ByteExt, capped};

use crate::error::{Error, Result};
use crate::records::{Record, RecordSet, StringRef, Value};
use crate::schema::{FieldType, Schema};

const HEADER_LEN: usize = 20;

/// The header after the `WDBC` magic.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub record_count: u32,
    pub field_count: u32,
    pub record_size: u32,
    pub string_block_size: u32,
}

/// A DBC whose header has been read and whose records and string block fit the file.
pub struct DbcParser<'a> {
    header: Header,
    body: &'a [u8],
    schema: Option<Schema>,
}

impl<'a> DbcParser<'a> {
    /// Reads the header from the start of the cursor's buffer, whatever its position.
    pub fn parse(cursor: &mut Cursor<&'a [u8]>) -> Result<Self> {
        let all: &'a [u8] = cursor.get_ref();
        if all.len() < HEADER_LEN || &all[..4] != b"WDBC" {
            return Err(Error::NotWdbc);
        }
        let field = |o| all.u32_at(o).ok_or(Error::Truncated("header"));
        let header = Header {
            record_count: field(4)?,
            field_count: field(8)?,
            record_size: field(12)?,
            string_block_size: field(16)?,
        };
        let body = &all[HEADER_LEN..];
        let layout = body_layout(&header).ok_or(Error::SizeOverflow)?;
        if body.len() < layout.end {
            return Err(Error::Truncated("records + string block"));
        }
        Ok(Self {
            header,
            body,
            schema: None,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Attaches the schema to decode records with; its field count must match the header's.
    pub fn with_schema(mut self, schema: Schema) -> Result<Self> {
        let n = schema.expanded_len();
        if n != self.header.field_count as usize {
            return Err(Error::SchemaFieldMismatch {
                schema: n,
                file: self.header.field_count,
            });
        }
        self.schema = Some(schema);
        Ok(self)
    }

    /// Decodes every record, copying the string block into the result.
    pub fn parse_records(&self) -> Result<RecordSet> {
        let schema = self.schema.as_ref().ok_or(Error::Truncated("no schema"))?;
        let rc = self.header.record_count as usize;
        let rs = self.header.record_size as usize;
        let fc = self.header.field_count as usize;
        let layout = body_layout(&self.header).ok_or(Error::SizeOverflow)?;
        let records_bytes = self
            .body
            .get(..layout.records_len)
            .ok_or(Error::Truncated("records"))?;
        let strings = self
            .body
            .get(layout.records_len..layout.end)
            .ok_or(Error::Truncated("string block"))?
            .to_vec();

        let mut types = Vec::with_capacity(fc);
        let mut names = Vec::with_capacity(fc);
        for field in &schema.fields {
            for k in 0..field.count {
                types.push(field.ty);
                names.push(if field.count == 1 {
                    field.name.clone()
                } else {
                    format!("{}[{k}]", field.name)
                });
            }
        }

        if rs == 0 && rc > 0 {
            return Err(Error::Truncated("records claimed at record_size 0"));
        }
        let rc = capped(rc, rs, records_bytes.len());
        let mut records = Vec::with_capacity(rc);
        for r in 0..rc {
            let base = r * rs;
            let values = types
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    let raw = u32_zero_padded(records_bytes, base + i * 4);
                    match ty {
                        FieldType::UInt32 => Value::UInt32(raw),
                        FieldType::Int32 => Value::Int32(raw as i32),
                        FieldType::Float32 => Value::Float32(f32::from_bits(raw)),
                        FieldType::String => Value::StringRef(StringRef(raw)),
                    }
                })
                .collect();
            records.push(Record { values });
        }

        Ok(RecordSet {
            field_names: names,
            records,
            strings,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BodyLayout {
    records_len: usize,
    end: usize,
}

fn body_layout(h: &Header) -> Option<BodyLayout> {
    checked_layout(
        h.record_count as usize,
        h.record_size as usize,
        h.string_block_size as usize,
    )
}

fn checked_layout(
    record_count: usize,
    record_size: usize,
    string_block_size: usize,
) -> Option<BodyLayout> {
    let records_len = record_count.checked_mul(record_size)?;
    let end = records_len.checked_add(string_block_size)?;
    Some(BodyLayout { records_len, end })
}

/// A field read by column offset, so one past its record's `record_size` reads on into the next
/// record, and one past the last record reads as zeros.
fn u32_zero_padded(b: &[u8], o: usize) -> u32 {
    let tail = b.get(o..).unwrap_or_default();
    let n = tail.len().min(4);
    let mut bytes = [0u8; 4];
    bytes[..n].copy_from_slice(&tail[..n]);
    u32::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests;
