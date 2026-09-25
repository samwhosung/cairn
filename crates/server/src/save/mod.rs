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

/// About once a minute, each player's position is saved if it moved; and always as it leaves.
const POSITIONS_EVERY_MS: u32 = 60_000;

/// Where a player stood, as the world's file keeps it: a map, a spot on it and a facing.
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

/// Whether the writer keeps its word, that a tick's results leave only once its changes are
/// durable. The others are controls that break it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Saving {
    #[default]
    Held,
    /// Each tick's results leave at once, and its changes are committed only once the next tick
    /// that changes anything has let its results out, so some result out is never durable.
    Early,
    /// The first player's row handed over at or after this tick is never written.
    Drops(u32),
}

/// A player the world knows: its number in the file, its name, where it last stood as saved, and
/// the game's saved fields of it, encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub id: u32,
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

/// Every player's saved state, by name.
pub type Keeping = BTreeMap<String, Kept>;

/// What one tick hands the writer: the players new to the world, the game's rows of players that
/// changed, and the places saved.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub tick: u32,
    pub joined: Vec<(u32, String)>,
    pub rows: Vec<(u32, Vec<u8>)>,
    pub places: Vec<(u32, Place)>,
}

impl Batch {
    pub fn rows(&self) -> usize {
        self.joined.len() + self.rows.len() + self.places.len()
    }
}

struct Known {
    player: Player,
    body: Option<u32>,
    here: bool,
}

/// The players the world knows by name, which body each has now, and what the tick saves.
pub struct Roster {
    known: Vec<Known>,
    by_name: HashMap<String, usize>,
    bodies: Vec<Option<usize>>,
    next_id: u32,
    map: u32,
    every: u32,
    batch: Batch,
}

/// A join the roster admits, and where the player comes back to if it saved a place on this map.
pub enum Admit {
    Refused,
    At(Option<Spawn>),
}

impl Roster {
    pub fn new(players: Vec<Player>, map: u32, tick_ms: u16) -> Self {
        let next_id = players.iter().map(|p| p.id + 1).max().unwrap_or(0);
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

    /// Admits a player named `name` unless one of that name is in the world.
    pub fn admit(&mut self, name: &str) -> Admit {
        let i = match self.by_name.get(name) {
            Some(&i) if self.known[i].here => return Admit::Refused,
            Some(&i) => i,
            None => {
                let id = self.next_id;
                self.next_id += 1;
                self.batch.joined.push((id, name.to_owned()));
                self.by_name.insert(name.to_owned(), self.known.len());
                self.known.push(Known {
                    player: Player {
                        id,
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
        let back = self.known[i].player.place.filter(|p| p.map == self.map);
        Admit::At(back.map(|p| Spawn {
            pos: p.pos,
            facing: p.facing,
        }))
    }

    /// Gives the player named `name` body `n`; what it saved, if anything, goes to `restored`.
    pub fn bind(&mut self, n: u32, name: &str, restored: &mut Vec<(u32, Vec<u8>)>) {
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
        if let Some(saved) = &self.known[i].player.saved {
            restored.push((n, saved.clone()));
        }
    }

    /// Body `n` left the world standing on `body`: its place is saved.
    pub fn leave(&mut self, n: u32, body: &Body) {
        let Some(i) = self.bodies.get(n as usize).copied().flatten() else {
            return;
        };
        self.known[i].here = false;
        self.place(i, Place::of(self.map, body));
    }

    /// The game's changed rows of players, by body.
    pub fn take(&mut self, record: &Record) {
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
                .rows
                .push((self.known[i].player.id, bytes.clone()));
            self.known[i].player.saved = Some(bytes.clone());
        }
    }

    /// Saves where each body due this tick stands, if it moved since it was last saved.
    pub fn due(&mut self, tick: u32, bodies: &[Body]) {
        let first = (tick % self.every) as usize;
        for n in (first..bodies.len()).step_by(self.every as usize) {
            if let (true, Some(&Some(i))) = (bodies[n].present, self.bodies.get(n)) {
                self.place(i, Place::of(self.map, &bodies[n]));
            }
        }
    }

    /// Saves where every body in the world stands, as the server stops.
    pub fn all(&mut self, bodies: &[Body]) {
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
            self.batch.places.push((player.id, at));
        }
    }

    /// What the tick saves, taken for the writer.
    pub fn batch(&mut self, tick: u32) -> Batch {
        let mut batch = std::mem::take(&mut self.batch);
        batch.tick = tick;
        batch
    }

    /// Every player's saved state as the world would have the file hold it: where it last stood
    /// as saved, and the game's saved fields of it from `scan`, every row of the world's; one not
    /// in the world this run keeps what it came with.
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
