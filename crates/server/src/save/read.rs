use std::fmt::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags};

const STEPS_BETWEEN_CHECKS: i32 = 1000;

/// Runs each statement on the world's file at `path` without writing to it, each in a read of its
/// own that gives up after `timeout`, as a live server keeps writing. Returns what they read: for
/// each, a line of its columns' names and a line a row, tab-separated.
pub fn read(path: &Path, statements: &[String], timeout: Duration) -> Result<String, String> {
    let at = |e: rusqlite::Error| format!("{}: {e}", path.display());
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(at)?;
    conn.busy_timeout(timeout).map_err(at)?;
    conn.pragma_update(None, "query_only", true).map_err(at)?;
    let mut out = String::new();
    for sql in statements {
        let deadline = Instant::now() + timeout;
        let past_it = move || Instant::now() >= deadline;
        conn.progress_handler(STEPS_BETWEEN_CHECKS, Some(past_it))
            .map_err(at)?;
        rows(&conn, sql, &mut out).map_err(|e| match e.sqlite_error_code() {
            Some(ErrorCode::OperationInterrupted) => format!(
                "{sql}: stopped after {} ms, the longest a read may take here",
                timeout.as_millis()
            ),
            _ => format!("{sql}: {e}"),
        })?;
    }
    Ok(out)
}

fn rows(conn: &Connection, sql: &str, out: &mut String) -> rusqlite::Result<()> {
    let mut s = conn.prepare(sql)?;
    let names: Vec<String> = s.column_names().iter().map(|n| (*n).to_owned()).collect();
    let _ = writeln!(out, "{}", names.join("\t"));
    let mut rows = s.query([])?;
    while let Some(row) = rows.next()? {
        let cells: Vec<String> = (0..names.len())
            .map(|i| row.get_ref(i).map(cell))
            .collect::<rusqlite::Result<_>>()?;
        let _ = writeln!(out, "{}", cells.join("\t"));
    }
    Ok(())
}

fn cell(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).replace(['\t', '\n'], " "),
        ValueRef::Blob(b) => b.iter().fold(String::from("x"), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }),
    }
}
