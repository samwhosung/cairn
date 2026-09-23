use std::io::Write;

use crate::records::{RecordSet, Value};

/// Writes a header row of column names, then a row per record with strings resolved; a string
/// that cannot be resolved is written empty. Cells are quoted as RFC 4180 requires.
pub fn export_to_csv<W: Write>(rs: &RecordSet, mut w: W) -> std::io::Result<()> {
    writeln!(w, "{}", rs.field_names.join(","))?;
    for record in &rs.records {
        let cells: Vec<String> = record
            .values
            .iter()
            .map(|v| match v {
                Value::UInt32(x) => x.to_string(),
                Value::Int32(x) => x.to_string(),
                Value::Float32(x) => x.to_string(),
                Value::StringRef(sr) => quote(&rs.get_string(*sr).unwrap_or_default()),
            })
            .collect();
        writeln!(w, "{}", cells.join(","))?;
    }
    Ok(())
}

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
