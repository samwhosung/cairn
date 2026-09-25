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
