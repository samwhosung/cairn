//! The world's saved state: players known by name, what each saved, and where each last stood.

mod file;
mod read;
mod writer;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use game::{Record, Schema, Value};

use crate::world::{Body, Spawn};

pub use file::{Opened, open, scan};
pub use read::read;
pub use writer::{Commit, Writer};

const POSITIONS_EVERY_MS: u32 = 60_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct Place {
    pub map: u32,
    pub pos: [f32; 3],
    pub facing: f32,
}

impl Place {
    fn of(map: u32, body: &Body) -> Self {
        Self {
            map,
            pos: body.movement.pos,
            facing: body.movement.facing,
        }
    }

    fn bits(&self) -> [u32; 5] {
        [
            self.map,
            self.pos[0].to_bits(),
            self.pos[1].to_bits(),
            self.pos[2].to_bits(),
            self.facing.to_bits(),
        ]
    }
}

impl PartialEq for Place {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}

/// How far a commit goes before it counts as made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flush {
    /// Past a loss of power.
    #[default]
    Drive,
    /// Into the system: past a crash of the process, not of the machine.
    System,
}

/// Whether the writer keeps its word, that a tick's results leave only once its changes are
/// committed. The others are controls that break it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Saving {
    #[default]
    Held,
    /// Each tick's results leave without waiting on its commit, which comes only once the next
    /// tick that saves anything has let its results out: from the first tick that saves, a kill
    /// always loses a tick whose results are out.
    Early,
    /// The first player's row handed over at or after this tick is never written.
    Drops(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub file_id: u32,
    pub name: String,
    pub place: Option<Place>,
    pub saved: Option<Vec<u8>>,
}

/// A player's saved state as the file holds it, or as the world would have it held.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub place: Option<Place>,
    pub saved: Option<Vec<Value>>,
}

pub type Keeping = BTreeMap<String, Kept>;

#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub tick: u32,
    pub new_players: Vec<(u32, String)>,
    pub game_rows: Vec<(u32, Vec<u8>)>,
    pub places: Vec<(u32, Place)>,
}

impl Batch {
    pub fn rows(&self) -> usize {
        self.new_players.len() + self.game_rows.len() + self.places.len()
    }
}

struct Known {
    player: Player,
    body: Option<u32>,
    here: bool,
}

pub struct Roster {
    known: Vec<Known>,
    by_name: HashMap<String, usize>,
    bodies: Vec<Option<usize>>,
    next_id: u32,
    map: u32,
    every: u32,
    batch: Batch,
}

pub enum Admit {
    Refused,
    AtSpawn,
    Back(Spawn),
}

impl Roster {
    pub fn new(players: Vec<Player>, map: u32, tick_ms: u16) -> Self {
        let next_id = players.iter().map(|p| p.file_id + 1).max().unwrap_or(0);
        let by_name = (0..)
            .zip(&players)
            .map(|(i, p)| (p.name.clone(), i))
            .collect();
        Self {
            known: players
                .into_iter()
                .map(|player| Known {
                    player,
                    body: None,
                    here: false,
                })
                .collect(),
            by_name,
            bodies: Vec::new(),
            next_id,
            map,
            every: (POSITIONS_EVERY_MS / u32::from(tick_ms.max(1))).max(1),
            batch: Batch::default(),
        }
    }

    pub fn players(&self) -> impl Iterator<Item = &Player> {
        self.known.iter().map(|k| &k.player)
    }

    pub fn admit(&mut self, name: &str) -> Admit {
        let i = match self.by_name.get(name) {
            Some(&i) if self.known[i].here => return Admit::Refused,
            Some(&i) => i,
            None => {
                let file_id = self.next_id;
                self.next_id += 1;
                self.batch.new_players.push((file_id, name.to_owned()));
                self.by_name.insert(name.to_owned(), self.known.len());
                self.known.push(Known {
                    player: Player {
                        file_id,
                        name: name.to_owned(),
                        place: None,
                        saved: None,
                    },
                    body: None,
                    here: false,
                });
                self.known.len() - 1
            }
        };
        self.known[i].here = true;
        match self.known[i].player.place.filter(|p| p.map == self.map) {
            Some(p) => Admit::Back(Spawn {
                pos: p.pos,
                facing: p.facing,
            }),
            None => Admit::AtSpawn,
        }
    }

    pub fn bind(&mut self, n: u32, name: &str) {
        let Some(&i) = self.by_name.get(name) else {
            return;
        };
        if let Some(old) = self.known[i].body.replace(n) {
            self.bodies[old as usize] = None;
        }
        if self.bodies.len() <= n as usize {
            self.bodies.resize(n as usize + 1, None);
        }
        self.bodies[n as usize] = Some(i);
    }

    pub fn saved(&self, name: &str) -> Option<&[u8]> {
        let &i = self.by_name.get(name)?;
        self.known[i].player.saved.as_deref()
    }

    pub fn leave(&mut self, n: u32, body: &Body) {
        let Some(i) = self.bodies.get(n as usize).copied().flatten() else {
            return;
        };
        self.known[i].here = false;
        self.place(i, Place::of(self.map, body));
    }

    pub fn take_player_rows(&mut self, record: &Record) {
        for (id, saved) in &record.saved {
            if !id.is_player() {
                continue;
            }
            let (Some(&Some(i)), Some(bytes)) = (self.bodies.get(id.n as usize), saved) else {
                continue;
            };
            if bytes.is_empty() {
                continue;
            }
            self.batch
                .game_rows
                .push((self.known[i].player.file_id, bytes.clone()));
            self.known[i].player.saved = Some(bytes.clone());
        }
    }

    pub fn save_due_places(&mut self, tick: u32, bodies: &[Body]) {
        let first = (tick % self.every) as usize;
        for n in (first..bodies.len()).step_by(self.every as usize) {
            if let (true, Some(&Some(i))) = (bodies[n].present, self.bodies.get(n)) {
                self.place(i, Place::of(self.map, &bodies[n]));
            }
        }
    }

    pub fn save_every_place(&mut self, bodies: &[Body]) {
        for (n, body) in bodies.iter().enumerate() {
            if let (true, Some(&Some(i))) = (body.present, self.bodies.get(n)) {
                self.place(i, Place::of(self.map, body));
            }
        }
    }

    fn place(&mut self, i: usize, at: Place) {
        let player = &mut self.known[i].player;
        if player.place != Some(at) {
            player.place = Some(at);
            self.batch.places.push((player.file_id, at));
        }
    }

    pub fn take_batch(&mut self, tick: u32) -> Batch {
        let mut batch = std::mem::take(&mut self.batch);
        batch.tick = tick;
        batch
    }

    pub fn keeping(&self, schema: Option<&Schema>, scan: &BTreeMap<game::Id, Vec<u8>>) -> Keeping {
        self.known
            .iter()
            .map(|k| {
                let bytes = match k.body {
                    Some(n) => scan.get(&game::Id::player(n)),
                    None => k.player.saved.as_ref(),
                };
                let saved = schema.zip(bytes).and_then(|(s, b)| s.values(b));
                let kept = Kept {
                    place: k.player.place,
                    saved,
                };
                (k.player.name.clone(), kept)
            })
            .collect()
    }
}

/// The file a world is kept in unless told otherwise: in the user's data directory, named after
/// the game it runs, or `world` with none.
pub fn default_world(game: Option<&str>) -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let data = if cfg!(target_os = "macos") {
        var("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else if cfg!(windows) {
        var("APPDATA").map(PathBuf::from)
    } else {
        var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/share")))
    }?;
    let name = format!("{}.sqlite", game.unwrap_or("world"));
    Some(data.join("cairn").join("worlds").join(name))
}

#[cfg(test)]
mod tests;
