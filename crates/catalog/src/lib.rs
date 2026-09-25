//! The games cairn carries, looked up by name for the server, the client and the bots.

use std::path::Path;

use game::{Line, Loaded};

pub const GAMES: [&str; 1] = ["melee"];

/// Game `name` on `knobs`, a knobs file's lines and its name, or on its own knobs without one,
/// with `over` laid on them.
pub fn load(
    name: &str,
    knobs: Option<(&[Line], &str)>,
    over: &[Line],
    seed: u64,
) -> Result<Loaded, String> {
    match name {
        "melee" => game::load::<melee::Melee>(knobs, over, seed),
        _ => Err(format!("no game `{name}`: {}", GAMES.join(", "))),
    }
}

/// The lines of the knobs file at `path`.
pub fn read(path: &Path) -> Result<Vec<Line>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    game::lines(&text, &path.display().to_string())
}

/// Game `name` on the knobs file at `knobs`, or on its own without one, with the file at
/// `overlay` laid on them.
pub fn from_files(
    name: &str,
    knobs: Option<&Path>,
    overlay: Option<&Path>,
    seed: u64,
) -> Result<Loaded, String> {
    let base = knobs
        .map(|p| read(p).map(|lines| (lines, p.display().to_string())))
        .transpose()?;
    let over = overlay.map(read).transpose()?.unwrap_or_default();
    let base = base
        .as_ref()
        .map(|(lines, file)| (&lines[..], file.as_str()));
    load(name, base, &over, seed)
}
