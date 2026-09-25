use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use game::{Schema, Tables, Value};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};

use super::{Batch, Keeping, Kept, Place, Player};

const LAYOUT_VERSION: i64 = 1;
const OWN: [&str; 3] = ["world", "player", "position"];
const WAL_LIMIT_BYTES: i64 = 64 << 20;

pub struct Opened {
    pub path: PathBuf,
    pub(super) conn: Connection,
    pub(super) _lock: File,
    pub(super) table: Option<Table>,
    pub players: Vec<Player>,
}

pub(super) struct Table {
    pub name: String,
    pub schema: Schema,
    upsert: String,
}

impl Table {
    fn new(schema: Schema) -> Self {
        let name = snake(schema.name);
        let fields: Vec<String> = schema.fields.iter().map(|f| quoted(f.name)).collect();
        let marks: Vec<String> = (2..=fields.len() + 1).map(|i| format!("?{i}")).collect();
        let sets: Vec<String> = fields
            .iter()
            .map(|f| format!("{f} = excluded.{f}"))
            .collect();
        let upsert = format!(
            "INSERT INTO {} (player, {}) VALUES (?1, {}) ON CONFLICT (player) DO UPDATE SET {}",
            quoted(&name),
            fields.join(", "),
            marks.join(", "),
            sets.join(", ")
        );
        Self {
            name,
            schema,
            upsert,
        }
    }
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn quoted(name: &str) -> String {
    format!("\"{name}\"")
}

fn at(path: &Path) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

/// Opens the world at `path`, making it if there is none, for a server running the game named
/// with its tables, or none. Refuses a file another server keeps, one of another game or of a
/// later layout, and one whose saved fields this build cannot read; a field of the players'
/// table added since the file was written, or the table itself, is added to it.
pub fn open(path: &Path, game: Option<(&str, &Tables)>) -> Result<Opened, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let lock = lock(path)?;
    let table = players_table(path, game)?;
    let mut conn = Connection::open(path).map_err(at(path))?;
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
        .map_err(at(path))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(format!("{}: keeps no write-ahead log", path.display()));
    }
    conn.execute_batch(&format!(
        "PRAGMA synchronous = FULL; PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON;
         PRAGMA wal_autocheckpoint = 0; PRAGMA journal_size_limit = {WAL_LIMIT_BYTES};
         PRAGMA busy_timeout = 5000;"
    ))
    .map_err(at(path))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(at(path))?;
    layout(&tx, path)?;
    let name = game.map(|g| g.0);
    claim_for_game(&tx, path, name)?;
    if let Some(table) = &table {
        columns(&tx, path, table, name.unwrap_or_default())?;
    }
    refuse_unknown_tables(&tx, path, table.as_ref(), name)?;
    tx.execute("UPDATE world SET value = value + 1 WHERE key = 'runs'", [])
        .map_err(at(path))?;
    tx.execute("UPDATE world SET value = NULL WHERE key = 'tick'", [])
        .map_err(at(path))?;
    let players = load(&tx, path, table.as_ref())?;
    tx.commit().map_err(at(path))?;
    Ok(Opened {
        path: path.to_path_buf(),
        conn,
        _lock: lock,
        table,
        players,
    })
}

fn lock(path: &Path) -> Result<File, String> {
    let mut name = path.as_os_str().to_owned();
    name.push("-lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&name)
        .map_err(|e| format!("{}: {e}", Path::new(&name).display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(format!(
            "{}: another server keeps this world",
            path.display()
        )),
        Err(TryLockError::Error(e)) => Err(format!("{}: {e}", path.display())),
    }
}

fn players_table(path: &Path, game: Option<(&str, &Tables)>) -> Result<Option<Table>, String> {
    let Some((name, tables)) = game else {
        return Ok(None);
    };
    if let Some(other) = tables.others.first() {
        return Err(format!(
            "{}: {name} saves `{}` rows of a kind other than its players', and a world keeps only \
             its players' yet",
            path.display(),
            other.name
        ));
    }
    let table = tables.players.map(Table::new);
    if let Some(t) = table.as_ref().filter(|t| OWN.contains(&t.name.as_str())) {
        return Err(format!(
            "{}: {name} saves its players in `{}`, a table of the world's own",
            path.display(),
            t.name
        ));
    }
    Ok(table)
}

fn layout(tx: &Transaction<'_>, path: &Path) -> Result<(), String> {
    let format: i64 = tx
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(at(path))?;
    if format > LAYOUT_VERSION {
        return Err(format!(
            "{}: a world of layout {format}, from a later build; this one reads layout {LAYOUT_VERSION}",
            path.display()
        ));
    }
    if format == LAYOUT_VERSION {
        return Ok(());
    }
    if !tables(tx, path)?.is_empty() {
        return Err(format!(
            "{}: holds tables, and is not a world",
            path.display()
        ));
    }
    tx.execute_batch(&format!(
        "CREATE TABLE world (key TEXT PRIMARY KEY, value ANY) STRICT;
         INSERT INTO world VALUES ('game', NULL), ('runs', 0), ('tick', NULL);
         CREATE TABLE player (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE) STRICT;
         CREATE TABLE position (player INTEGER PRIMARY KEY, map INTEGER NOT NULL,
             x REAL NOT NULL, y REAL NOT NULL, z REAL NOT NULL, facing REAL NOT NULL) STRICT;
         PRAGMA user_version = {LAYOUT_VERSION};"
    ))
    .map_err(at(path))
}

fn tables(tx: &Transaction<'_>, path: &Path) -> Result<Vec<String>, String> {
    let mut s = tx
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .map_err(at(path))?;
    let names = s
        .query_map([], |r| r.get(0))
        .map_err(at(path))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(at(path))?;
    Ok(names)
}

fn claim_for_game(tx: &Transaction<'_>, path: &Path, game: Option<&str>) -> Result<(), String> {
    let was: Option<String> = tx
        .query_row("SELECT value FROM world WHERE key = 'game'", [], |r| {
            r.get(0)
        })
        .map_err(at(path))?;
    match (was.as_deref(), game) {
        (Some(was), Some(now)) if was == now => Ok(()),
        (Some(was), now) => Err(format!(
            "{}: a world of {was}, and this server runs {}",
            path.display(),
            now.unwrap_or("no game")
        )),
        (None, Some(now)) => tx
            .execute("UPDATE world SET value = ?1 WHERE key = 'game'", [now])
            .map(drop)
            .map_err(at(path)),
        (None, None) => Ok(()),
    }
}

struct Have {
    name: String,
    sql: String,
    not_null: bool,
    key: bool,
}

fn columns(tx: &Transaction<'_>, path: &Path, table: &Table, game: &str) -> Result<(), String> {
    let name = &table.name;
    let mut s = tx
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)")
        .map_err(at(path))?;
    let have = s
        .query_map([name], |r| {
            Ok(Have {
                name: r.get(0)?,
                sql: r.get(1)?,
                not_null: r.get(2)?,
                key: r.get::<_, i64>(3)? == 1,
            })
        })
        .map_err(at(path))?
        .collect::<Result<Vec<Have>, _>>()
        .map_err(at(path))?;
    let fields = table.schema.fields;
    let defaults = table.schema.defaults();
    if have.is_empty() {
        let columns: Vec<String> = fields
            .iter()
            .zip(&defaults)
            .map(|(f, d)| column(f, *d))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {game}: {e}", path.display()))?;
        let sql = format!(
            "CREATE TABLE {} (player INTEGER PRIMARY KEY, {}) STRICT",
            quoted(name),
            columns.join(", ")
        );
        return tx.execute_batch(&sql).map_err(at(path));
    }
    if !have
        .first()
        .is_some_and(|c| c.name == "player" && c.key && c.sql == "INTEGER")
    {
        return Err(format!(
            "{}: `{name}` is not kept by player",
            path.display()
        ));
    }
    for c in &have[1..] {
        let Some(f) = fields.iter().find(|f| f.name == c.name) else {
            return Err(format!(
                "{}: `{name}.{}` is saved there, and this build of {game} saves no such field: \
                 the file is from a later build",
                path.display(),
                c.name
            ));
        };
        let now = kind(f.sql.name(), !f.nullable);
        if kind(&c.sql, c.not_null) != now {
            return Err(format!(
                "{}: `{name}.{}` holds {}, and {game} saves it as {now}: a field that changes \
                 what it holds needs a migration written for it",
                path.display(),
                c.name,
                kind(&c.sql, c.not_null)
            ));
        }
    }
    for (f, d) in fields.iter().zip(&defaults) {
        if have.iter().all(|c| c.name != f.name) {
            let column = column(f, *d).map_err(|e| format!("{}: {game}: {e}", path.display()))?;
            let sql = format!("ALTER TABLE {} ADD COLUMN {column}", quoted(name));
            tx.execute_batch(&sql).map_err(at(path))?;
        }
    }
    Ok(())
}

fn kind(sql: &str, not_null: bool) -> String {
    if not_null {
        format!("{sql} NOT NULL")
    } else {
        sql.to_owned()
    }
}

fn column(f: &game::Field, default: Value) -> Result<String, String> {
    let literal = match default {
        Value::Null => "NULL".to_owned(),
        Value::Integer(v) => v.to_string(),
        Value::Real(v) if v.is_finite() => format!("{v:?}"),
        Value::Real(v) => {
            return Err(format!(
                "`{}` defaults to {v}, which a file cannot hold",
                f.name
            ));
        }
    };
    let null = if f.nullable { "" } else { " NOT NULL" };
    Ok(format!(
        "{} {}{null} DEFAULT {literal}",
        quoted(f.name),
        f.sql.name()
    ))
}

fn refuse_unknown_tables(
    tx: &Transaction<'_>,
    path: &Path,
    table: Option<&Table>,
    game: Option<&str>,
) -> Result<(), String> {
    for name in tables(tx, path)? {
        if OWN.contains(&name.as_str()) || table.is_some_and(|t| t.name == name) {
            continue;
        }
        return Err(format!(
            "{}: keeps `{name}`, which {} does not save: the file is from a later build",
            path.display(),
            game.unwrap_or("a world with no game")
        ));
    }
    Ok(())
}

fn load(tx: &Transaction<'_>, path: &Path, table: Option<&Table>) -> Result<Vec<Player>, String> {
    let (fields, join) = match table {
        Some(t) => {
            let names: Vec<String> = t
                .schema
                .fields
                .iter()
                .map(|f| format!("g.{}", quoted(f.name)))
                .collect();
            (
                format!(", g.player IS NOT NULL, {}", names.join(", ")),
                format!(" LEFT JOIN {} g ON g.player = p.id", quoted(&t.name)),
            )
        }
        None => (String::new(), String::new()),
    };
    let sql = format!(
        "SELECT p.id, p.name, q.map, q.x, q.y, q.z, q.facing{fields} FROM player p \
         LEFT JOIN position q ON q.player = p.id{join} ORDER BY p.id"
    );
    let mut s = tx.prepare(&sql).map_err(at(path))?;
    let mut rows = s.query([]).map_err(at(path))?;
    let mut players = Vec::new();
    while let Some(r) = rows.next().map_err(at(path))? {
        players.push(player(r, table).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    Ok(players)
}

fn player(r: &rusqlite::Row<'_>, table: Option<&Table>) -> Result<Player, String> {
    let sql = |e: rusqlite::Error| e.to_string();
    let name: String = r.get(1).map_err(sql)?;
    let bad = |what: String| format!("player `{name}`: {what}");
    let file_id = u32::try_from(r.get::<_, i64>(0).map_err(sql)?)
        .map_err(|_| bad("its number is out of range".into()))?;
    let place = match r.get::<_, Option<i64>>(2).map_err(sql)? {
        Some(map) => {
            let real = |i: usize| r.get::<_, f64>(i).map(|v| v as f32).map_err(sql);
            Some(Place {
                map: u32::try_from(map).map_err(|_| bad("its map is out of range".into()))?,
                pos: [real(3)?, real(4)?, real(5)?],
                facing: real(6)?,
            })
        }
        None => None,
    };
    let saved = match table {
        Some(t) if r.get::<_, bool>(7).map_err(sql)? => {
            let values = (8..8 + t.schema.fields.len())
                .map(|i| r.get_ref(i).map(value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            let bytes = t.schema.bytes(&values).ok_or_else(|| {
                bad(format!(
                    "`{}` holds {values:?}, which its fields cannot",
                    t.name
                ))
            })?;
            Some(bytes)
        }
        _ => None,
    };
    Ok(Player {
        file_id,
        name,
        place,
        saved,
    })
}

fn value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Integer(i) => Value::Integer(i),
        ValueRef::Real(f) => Value::Real(f),
        ValueRef::Null | ValueRef::Text(_) | ValueRef::Blob(_) => Value::Null,
    }
}

fn sql(v: Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Integer(i) => rusqlite::types::Value::Integer(i),
        Value::Real(f) => rusqlite::types::Value::Real(f),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Wrote {
    pub rows: u32,
    pub value_bytes: u32,
}

/// Writes `batch` in one transaction, durable once this returns, leaving out `skip` rows of the
/// game's.
pub(super) fn write(
    conn: &mut Connection,
    table: Option<&Table>,
    batch: &Batch,
    skip: usize,
) -> rusqlite::Result<Wrote> {
    let mut wrote = Wrote::default();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut s = tx.prepare_cached("INSERT INTO player (id, name) VALUES (?1, ?2)")?;
        for (id, name) in &batch.new_players {
            s.execute(params![id, name])?;
            wrote.rows += 1;
            wrote.value_bytes += 8 + name.len() as u32;
        }
        if let Some(t) = table {
            let mut s = tx.prepare_cached(&t.upsert)?;
            for (id, bytes) in batch.game_rows.iter().skip(skip) {
                let values = t.schema.values(bytes).ok_or_else(|| {
                    rusqlite::Error::ToSqlConversionFailure(
                        format!("player {id}: its saved fields do not read as `{}`", t.name).into(),
                    )
                })?;
                let all = std::iter::once(rusqlite::types::Value::Integer(i64::from(*id)))
                    .chain(values.into_iter().map(sql));
                s.execute(rusqlite::params_from_iter(all))?;
                wrote.rows += 1;
                wrote.value_bytes += 8 * (1 + t.schema.fields.len() as u32);
            }
        }
        let mut s = tx.prepare_cached(
            "INSERT INTO position (player, map, x, y, z, facing) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (player) DO UPDATE SET map = excluded.map, x = excluded.x, \
             y = excluded.y, z = excluded.z, facing = excluded.facing",
        )?;
        for (id, p) in &batch.places {
            let [x, y, z] = p.pos.map(f64::from);
            s.execute(params![id, p.map, x, y, z, f64::from(p.facing)])?;
            wrote.rows += 1;
            wrote.value_bytes += 48;
        }
        tx.prepare_cached("UPDATE world SET value = ?1 WHERE key = 'tick'")?
            .execute([batch.tick])?;
    }
    tx.commit()?;
    Ok(wrote)
}

/// Every player's saved state in the world's file at `path`, read through a connection of its
/// own: a full scan, for comparing with the world.
pub fn scan(path: &Path, schema: Option<&Schema>) -> Result<Keeping, String> {
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(at(path))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(at(path))?;
    let table = schema.map(|s| Table::new(*s));
    let tx = conn.unchecked_transaction().map_err(at(path))?;
    let players = load(&tx, path, table.as_ref())?;
    Ok(players
        .into_iter()
        .map(|p| {
            let saved = table
                .as_ref()
                .zip(p.saved.as_deref())
                .and_then(|(t, b)| t.schema.values(b));
            (
                p.name,
                Kept {
                    place: p.place,
                    saved,
                },
            )
        })
        .collect())
}
