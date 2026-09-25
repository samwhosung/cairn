//! The games cairn carries, looked up by name for the server, the client and the bots.

use std::path::Path;

use game::{KnobsFile, Line, Loaded};

pub const GAMES: [&str; 1] = ["melee"];

pub fn load(
    name: &str,
    base: Option<&KnobsFile>,
    over: &[Line],
    seed: u64,
) -> Result<Loaded, String> {
    match name {
        "melee" => game::load::<melee::Melee>(base, over, seed),
        _ => Err(format!("no game `{name}`: {}", GAMES.join(", "))),
    }
}

pub fn read(path: &Path) -> Result<KnobsFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    KnobsFile::parse(&text, &path.display().to_string())
}

/// Game `name` on the knobs file at `knobs`, or on its own without one, with the file at
/// `overlay` laid on them.
pub fn from_files(
    name: &str,
    knobs: Option<&Path>,
    overlay: Option<&Path>,
    seed: u64,
) -> Result<Loaded, String> {
    let base = knobs.map(read).transpose()?;
    let over = overlay.map(read).transpose()?.unwrap_or_default();
    load(name, base.as_ref(), &over.lines, seed)
}
