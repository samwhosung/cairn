//! Applying a command to a zone: each checks all it can before it changes anything, and returns
//! what it says it did and the image of its footprint from before it.

mod ground;
mod paint;
mod scatter;

use std::fmt::Write as _;

use crate::command::{Command, Height, Move, New, Place, Set};
use crate::frame::{Frame, compass};
use crate::image::{Effect, Image};
use crate::install::Install;
use crate::text::{centi, from_centi, scale_text, scale_u16, two_places};
use crate::zone::{
    AUTHOR_LIMIT, Author, Id, MADE_LIMIT, Start, Texture, Thing, Water, Z, Zone, map_dir,
};

pub struct Applied {
    pub reply: String,
    pub before: Image,
}

pub fn create(n: &New, install: &mut dyn Install) -> Result<(Zone, String), String> {
    if !install.has_texture(&n.texture)? {
        return Err(format!("{}: no such texture in the install", n.texture));
    }
    let frame = Frame {
        origin: n.origin,
        size: n.tiles,
    };
    let texture = Texture {
        path: n.texture.clone(),
        effect: n.effect,
    };
    let mut z = Zone::new(&n.name, frame, n.height as f32, texture);
    z.settings.borrow = Some(install.zone_named(&n.borrow)?);
    let e = frame.extent();
    let reply = format!(
        "new zone {}: {}×{} tiles, {} × {} yd (x east 0..{}, y south 0..{}), flat at {} yd",
        n.name,
        n.tiles.0,
        n.tiles.1,
        two_places(e[0]),
        two_places(e[1]),
        two_places(e[0]),
        two_places(e[1]),
        two_places(n.height)
    );
    Ok((z, reply))
}

/// Apply `cmd` for `author`. On an error the zone is as it was.
pub fn apply(
    z: &mut Zone,
    author: &str,
    cmd: &Command,
    install: &mut dyn Install,
) -> Result<Applied, String> {
    let (authors, palette) = (z.authors.clone(), z.palette.len());
    let done = change(z, author, cmd, install);
    if done.is_err() {
        z.authors = authors;
        z.palette.truncate(palette);
    }
    let (reply, before) = done?;
    Ok(Applied { reply, before })
}

fn change(
    z: &mut Zone,
    author: &str,
    cmd: &Command,
    install: &mut dyn Install,
) -> Result<(String, Image), String> {
    match cmd {
        Command::New(_) => Err("a zone is made only once, by its first command".into()),
        Command::Set(s) => set(z, s, install),
        Command::Ground(g) => ground::apply(z, g),
        Command::Paint(p) => {
            let t = texture(z, &p.texture, install)?;
            let (mut reply, image) = paint::apply(z, t.place, p);
            if t.added {
                let _ = write!(
                    reply,
                    "; {} is new to the zone, with no ground effect (`texture PATH --effect N` \
                     gives it one)",
                    z.palette[usize::from(t.place)].path
                );
            }
            Ok((reply, image))
        }
        Command::Texture { path, effect } => {
            let place = texture(z, path, install)?.place;
            let entry = &mut z.palette[usize::from(place)];
            let before = Image {
                effects: vec![Effect {
                    place,
                    effect: entry.effect,
                }],
                ..Image::default()
            };
            entry.effect = *effect;
            Ok((
                format!("{} takes ground effect {effect}", entry.path),
                before,
            ))
        }
        Command::Place(p) => place(z, author, p, install),
        Command::Move(m) => shift(z, m),
        Command::Remove(ids) => {
            if let Some(id) = ids.iter().find(|id| !z.things.contains_key(id)) {
                return Err(format!("no thing {id}"));
            }
            let mut before = Image::default();
            for id in ids {
                if let Some(t) = z.things.remove(id) {
                    before.things.push((id.clone(), Some(t)));
                }
            }
            Ok((format!("{} removed", before.things.len()), before))
        }
        Command::Scatter(sc) => {
            for m in &sc.models {
                if m.to_ascii_lowercase().ends_with(".wmo") {
                    return Err(format!("{m}: scatter spreads models, not buildings"));
                }
                install.model_box(m)?;
            }
            let things = scatter::spots(z, sc)?;
            let ids = allocate(z, author, things.len())?;
            let mut before = Image::default();
            for (id, t) in ids.iter().zip(&things) {
                before.things.push((id.clone(), None));
                z.things.insert(id.clone(), t.clone());
            }
            Ok((scatter::reply(z, sc, &ids, &things), before))
        }
        Command::Water { level, area } => {
            let [lo, hi] = area.bounds();
            let e = z.frame.extent();
            if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] > e[0] || lo[1] > e[1] {
                return Err("the water's outline is off the zone".into());
            }
            let w = Water {
                level: centi(*level),
                shape: to_hundredths(area),
            };
            let id = allocate(z, author, 1)?.remove(0);
            z.water.insert(id.clone(), w);
            let before = Image {
                water: vec![(id.clone(), None)],
                ..Image::default()
            };
            Ok((
                format!("water {id} at {} yd", from_centi(centi(*level))),
                before,
            ))
        }
        Command::Dry(ids) => {
            if let Some(id) = ids.iter().find(|id| !z.water.contains_key(id)) {
                return Err(format!("no water {id}"));
            }
            let mut before = Image::default();
            for id in ids {
                if let Some(w) = z.water.remove(id) {
                    before.water.push((id.clone(), Some(w)));
                }
            }
            Ok((format!("{} water removed", before.water.len()), before))
        }
    }
}

struct InPalette {
    place: u16,
    added: bool,
}

/// A texture stays in the palette once it is there, so paint always names a place it has.
fn texture(z: &mut Zone, path: &str, install: &mut dyn Install) -> Result<InPalette, String> {
    if let Some(place) = z.palette_place(path) {
        return Ok(InPalette {
            place,
            added: false,
        });
    }
    if !install.has_texture(path)? {
        return Err(format!("{path}: no such texture in the install"));
    }
    if z.palette.len() >= usize::from(u16::MAX) {
        return Err("the palette is full".into());
    }
    z.palette.push(Texture {
        path: path.replace('/', "\\"),
        effect: 0,
    });
    Ok(InPalette {
        place: (z.palette.len() - 1) as u16,
        added: true,
    })
}

/// The next `n` ids of `author`, who joins the zone's authors on their first.
fn allocate(z: &mut Zone, author: &str, n: usize) -> Result<Vec<Id>, String> {
    #[cfg(test)]
    if control::SHARED.with(std::cell::Cell::get) {
        return Ok(control::largest_plus_one(z, n));
    }
    let at = if let Some(i) = z.authors.iter().position(|a| a.name == author) {
        i
    } else {
        if z.authors.len() >= AUTHOR_LIMIT {
            return Err(format!("the zone has its {AUTHOR_LIMIT} authors already"));
        }
        z.authors.push(Author {
            name: author.to_owned(),
            made: 0,
        });
        z.authors.len() - 1
    };
    let a = &mut z.authors[at];
    let first = a.made;
    let last = u32::try_from(n)
        .ok()
        .and_then(|n| first.checked_add(n))
        .filter(|&l| l <= MADE_LIMIT)
        .ok_or_else(|| format!("{author} has handed out every id an author can"))?;
    a.made = last;
    Ok((first + 1..=last)
        .map(|n| Id {
            author: author.to_owned(),
            n,
        })
        .collect())
}

fn to_hundredths(s: &crate::shape::Shape) -> crate::shape::Shape {
    use crate::shape::Shape;
    let q = |v: f64| centi(v) as f64 / 100.0;
    let qp = |p: &[f64; 2]| [q(p[0]), q(p[1])];
    match s {
        Shape::Circle { c, r } => Shape::Circle { c: qp(c), r: q(*r) },
        Shape::Line { pts, z, width } => Shape::Line {
            pts: pts.iter().map(qp).collect(),
            z: z.iter().map(|z| z.map(q)).collect(),
            width: q(*width),
        },
        Shape::Rect { a, b } => Shape::Rect { a: qp(a), b: qp(b) },
        Shape::Poly { pts } => Shape::Poly {
            pts: pts.iter().map(qp).collect(),
        },
    }
}

fn z_of(h: Height) -> Z {
    match h {
        Height::Above(d) => Z::Ground(centi(d)),
        Height::At(h) => Z::At(centi(h)),
    }
}

fn off_zone(z: &Zone, p: [f64; 2]) -> String {
    let e = z.frame.extent();
    format!(
        "{},{} is off the zone (0..{} east, 0..{} south)",
        two_places(p[0]),
        two_places(p[1]),
        two_places(e[0]),
        two_places(e[1])
    )
}

fn place(
    z: &mut Zone,
    author: &str,
    p: &Place,
    install: &mut dyn Install,
) -> Result<(String, Image), String> {
    let size = install.model_box(&p.model)?.size();
    if !z.frame.contains(p.at) {
        return Err(off_zone(z, p.at));
    }
    let t = Thing {
        model: p.model.replace('/', "\\"),
        x: centi(p.at[0]),
        y: centi(p.at[1]),
        z: z_of(p.z),
        facing: centi(p.facing.rem_euclid(360.0)),
        scale: scale_u16(p.scale)?,
        set: p.set,
    };
    let id = allocate(z, author, 1)?.remove(0);
    let k = t.scale_f();
    let reply = format!(
        "placed {id} at {},{} z {} facing {} ({}); its box {} × {} × {} yd at scale {}",
        two_places(p.at[0]),
        two_places(p.at[1]),
        two_places(t.world_z(&z.heights)),
        two_places(t.facing_deg()),
        compass(t.facing_deg()),
        two_places(f64::from(size[0]) * k),
        two_places(f64::from(size[1]) * k),
        two_places(f64::from(size[2]) * k),
        scale_text(t.scale)
    );
    z.things.insert(id.clone(), t);
    Ok((
        reply,
        Image {
            things: vec![(id, None)],
            ..Image::default()
        },
    ))
}

fn shift(z: &mut Zone, m: &Move) -> Result<(String, Image), String> {
    let old = z
        .things
        .get(&m.id)
        .cloned()
        .ok_or_else(|| format!("no thing {}", m.id))?;
    let mut t = old.clone();
    if let Some(p) = m.to {
        if !z.frame.contains(p) {
            return Err(off_zone(z, p));
        }
        t.x = centi(p[0]);
        t.y = centi(p[1]);
    }
    if let Some(f) = m.facing {
        t.facing = centi(f.rem_euclid(360.0));
    }
    if let Some(d) = m.turn {
        t.facing = centi((t.facing_deg() + d).rem_euclid(360.0));
    }
    if let Some(s) = m.scale {
        t.scale = scale_u16(s)?;
    }
    if let Some(h) = m.z {
        t.z = z_of(h);
    }
    if let Some(s) = m.set {
        if t.set.is_none() {
            return Err("--set is a building's doodad set".into());
        }
        t.set = Some(s);
    }
    let reply = format!(
        "{} now at {},{} z {} facing {} scale {}",
        m.id,
        two_places(t.at()[0]),
        two_places(t.at()[1]),
        two_places(t.world_z(&z.heights)),
        two_places(t.facing_deg()),
        scale_text(t.scale)
    );
    z.things.insert(m.id.clone(), t);
    Ok((
        reply,
        Image {
            things: vec![(m.id.clone(), Some(old))],
            ..Image::default()
        },
    ))
}

fn set(z: &mut Zone, s: &Set, install: &mut dyn Install) -> Result<(String, Image), String> {
    let mut next = z.settings.clone();
    if let Some(n) = &s.name {
        next.name.clone_from(n);
        next.map = map_dir(n);
    }
    if let Some([x, y, f]) = s.start {
        if !z.frame.contains([x, y]) {
            return Err("the start is off the zone".into());
        }
        next.start = Some(Start {
            x: centi(x),
            y: centi(y),
            facing: centi(f),
        });
    }
    if let Some(b) = &s.borrow {
        next.borrow = Some(install.zone_named(b)?);
    }
    let before = Image {
        settings: Some(std::mem::replace(&mut z.settings, next)),
        ..Image::default()
    };
    let s = &z.settings;
    let start = s.start.map_or("-".into(), |st| {
        format!(
            "{},{} facing {}",
            from_centi(st.x),
            from_centi(st.y),
            from_centi(st.facing)
        )
    });
    let borrows = s.borrow.as_ref().map_or("nothing", |b| b.name.as_str());
    Ok((
        format!("zone {}: start {start}, borrows {borrows}", s.name),
        before,
    ))
}

#[cfg(test)]
pub mod control {
    use std::cell::Cell;

    use crate::zone::{Id, Zone};

    thread_local!(pub static SHARED: Cell<bool> = const { Cell::new(false) });

    /// The two authors' check's control, on this thread: every author's ids are the largest in
    /// the zone plus one, as one shared counter would hand them out.
    pub fn share_ids() {
        SHARED.with(|s| s.set(true));
    }

    pub fn largest_plus_one(z: &Zone, n: usize) -> Vec<Id> {
        let top = z
            .things
            .keys()
            .chain(z.water.keys())
            .map(|id| id.n)
            .max()
            .unwrap_or(0);
        (top + 1..=top + n as u32)
            .map(|n| Id {
                author: "shared".into(),
                n,
            })
            .collect()
    }
}
