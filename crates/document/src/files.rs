//! A zone's own files, in its directory:
//!
//! - `zone.txt`: its name, size, where it lies on its map, the start, the zone it borrows from,
//!   and each author with the ids they have handed out;
//! - `things.txt`, `water.txt`, `palette.txt`: a line for each thing, body of water and texture;
//! - `heights.bin`: every vertex's height, the outer lattice then the inner, as f32;
//! - `paint.bin`: per chunk, each texture's weight on each texel.
//!
//! They hold the zone as of a line of its journal, which `.saved` names. A save writes each file
//! beside its old one, names them all in `.saving`, then moves them in: a save cut short is
//! finished or forgotten when the zone is next opened, never half kept.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::frame::{Frame, MAP_TILES, MAX_ZONE_TILES};
use crate::text::{
    centi, from_centi, lines, number, parse_id, parse_thing, parse_water, thing_text, water_text,
};
use crate::zone::{
    Author, Borrow, ChunkPaint, Heights, Id, MAX_LAYERS, PaintLayer, Settings, Start,
    TEXELS_IN_CHUNK, Texture, Zone, check_author,
};

const HEIGHTS_MAGIC: &[u8; 4] = b"ZHGT";
const PAINT_MAGIC: &[u8; 4] = b"ZPNT";
pub const SAVED: &str = ".saved";
const SAVING: &str = ".saving";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ZoneFile {
    Zone,
    Heights,
    Paint,
    Palette,
    Things,
    Water,
}

impl ZoneFile {
    pub const ALL: [ZoneFile; 6] = [
        ZoneFile::Zone,
        ZoneFile::Heights,
        ZoneFile::Paint,
        ZoneFile::Palette,
        ZoneFile::Things,
        ZoneFile::Water,
    ];

    pub fn file(self) -> &'static str {
        match self {
            ZoneFile::Zone => "zone.txt",
            ZoneFile::Heights => "heights.bin",
            ZoneFile::Paint => "paint.bin",
            ZoneFile::Palette => "palette.txt",
            ZoneFile::Things => "things.txt",
            ZoneFile::Water => "water.txt",
        }
    }

    fn from_file(f: &str) -> Option<ZoneFile> {
        ZoneFile::ALL.into_iter().find(|p| p.file() == f)
    }

    pub(crate) fn bytes(self, z: &Zone) -> Vec<u8> {
        match self {
            ZoneFile::Zone => settings_text(z).into_bytes(),
            ZoneFile::Heights => heights_bytes(&z.heights),
            ZoneFile::Paint => paint_bytes(&z.paint),
            ZoneFile::Palette => palette_text(&z.palette).into_bytes(),
            ZoneFile::Things => {
                let mut s = String::from(
                    "# id x y z facing scale set model: zone yards (x east, y south); z +d above \
                     the ground or =h a world height; facing the compass bearing of the model's \
                     front; set a building's doodad set\n",
                );
                for (id, t) in &z.things {
                    let _ = writeln!(s, "{id} {}", thing_text(t));
                }
                s.into_bytes()
            }
            ZoneFile::Water => {
                let mut s = String::from("# id type level outline (zone yards: x east, y south)\n");
                for (id, w) in &z.water {
                    let _ = writeln!(s, "{id} {}", water_text(w));
                }
                s.into_bytes()
            }
        }
    }
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

fn beside(dir: &Path, file: &str) -> std::path::PathBuf {
    dir.join(format!(".{file}.new"))
}

pub fn save(dir: &Path, z: &Zone, parts: &[ZoneFile], line: usize) -> Result<(), String> {
    let mut saving = format!("{line}\n");
    for &p in parts {
        let tmp = beside(dir, p.file());
        std::fs::write(&tmp, p.bytes(z)).map_err(io(&tmp))?;
        let _ = writeln!(saving, "{}", p.file());
    }
    let tmp = beside(dir, SAVING);
    std::fs::write(&tmp, saving).map_err(io(&tmp))?;
    std::fs::rename(&tmp, dir.join(SAVING)).map_err(io(&tmp))?;
    finish(dir)
}

/// Move a save named in `.saving` in, and forget the pieces of one that never got that far.
pub fn finish(dir: &Path) -> Result<(), String> {
    let saving = dir.join(SAVING);
    if let Ok(list) = std::fs::read_to_string(&saving) {
        for f in list.lines().skip(1).filter_map(ZoneFile::from_file) {
            let tmp = beside(dir, f.file());
            if tmp.exists() {
                std::fs::rename(&tmp, dir.join(f.file())).map_err(io(&tmp))?;
            }
        }
        std::fs::rename(&saving, dir.join(SAVED)).map_err(io(&saving))?;
    }
    for p in ZoneFile::ALL {
        let _ = std::fs::remove_file(beside(dir, p.file()));
    }
    let _ = std::fs::remove_file(beside(dir, SAVING));
    Ok(())
}

pub fn saved_line(dir: &Path) -> Result<usize, String> {
    let path = dir.join(SAVED);
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    let first = text.lines().next().unwrap_or_default();
    number(first, &format!("{}'s line", path.display()))
}

pub fn load(dir: &Path) -> Result<Zone, String> {
    let read = |f: &str| {
        let path = dir.join(f);
        std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{}: no {f} (is this a zone? `cairn zone new` makes one)",
                dir.display()
            ),
            _ => format!("{}: {e}", path.display()),
        })
    };
    let text = |f: &str| String::from_utf8(read(f)?).map_err(|_| format!("{f}: not text"));
    let within = |f: &str, e: String| format!("{}: {e}", dir.join(f).display());
    let (frame, settings, authors) =
        parse_settings(&text("zone.txt")?).map_err(|e| within("zone.txt", e))?;
    let (cols, rows) = frame.cells();
    let heights =
        read_heights(&read("heights.bin")?, cols, rows).map_err(|e| within("heights.bin", e))?;
    let palette = parse_palette(&text("palette.txt")?).map_err(|e| within("palette.txt", e))?;
    let (east, south) = frame.chunks();
    let paint = read_paint(&read("paint.bin")?, east * south, palette.len())
        .map_err(|e| within("paint.bin", e))?;
    let things = parse_by_id(&text("things.txt")?, &authors, parse_thing)
        .map_err(|e| within("things.txt", e))?;
    let water = parse_by_id(&text("water.txt")?, &authors, parse_water)
        .map_err(|e| within("water.txt", e))?;
    Ok(Zone {
        frame,
        settings,
        authors,
        heights,
        paint,
        palette,
        things,
        water,
    })
}

/// Lines of `ID REST`, each id one its author has handed out, and none twice.
fn parse_by_id<T>(
    text: &str,
    authors: &[Author],
    parse: fn(&str) -> Result<T, String>,
) -> Result<BTreeMap<Id, T>, String> {
    let mut out = BTreeMap::new();
    for (n, l) in lines(text) {
        let at = |e: String| format!("line {n}: {e}");
        let (id, rest) = l
            .split_once(char::is_whitespace)
            .ok_or_else(|| at("no id".into()))?;
        let id = parse_id(id).map_err(at)?;
        if !authors
            .iter()
            .any(|a| a.name == id.author && id.n <= a.made)
        {
            return Err(at(format!("{id} is not an id its author has handed out")));
        }
        if out.insert(id.clone(), parse(rest).map_err(at)?).is_some() {
            return Err(at(format!("{id} again")));
        }
    }
    Ok(out)
}

fn settings_text(z: &Zone) -> String {
    let s = &z.settings;
    let f = &z.frame;
    let mut t = format!(
        "name {}\nmap {}\nsize {} {}\norigin {} {}\n",
        s.name, s.map, f.size.0, f.size.1, f.origin.0, f.origin.1
    );
    if let Some(st) = s.start {
        let _ = writeln!(
            t,
            "start {} {} {}",
            from_centi(st.x),
            from_centi(st.y),
            from_centi(st.facing)
        );
    }
    if let Some(b) = &s.borrow {
        let _ = writeln!(t, "borrow {} {}", b.area, b.name);
    }
    for a in &z.authors {
        let _ = writeln!(t, "author {} {}", a.name, a.made);
    }
    t
}

fn parse_settings(s: &str) -> Result<(Frame, Settings, Vec<Author>), String> {
    let mut settings = Settings {
        name: String::new(),
        map: String::new(),
        start: None,
        borrow: None,
    };
    let (mut size, mut origin, mut authors) = (None, None, Vec::new());
    let two = |v: &str| -> Result<(u32, u32), String> {
        match v.split_whitespace().collect::<Vec<_>>()[..] {
            [a, b] => Ok((number(a, "a number")?, number(b, "a number")?)),
            _ => Err(format!("want two numbers, not {v:?}")),
        }
    };
    for (n, l) in lines(s) {
        let (k, v) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        let v = v.trim();
        let at = |e: String| format!("line {n}: {e}");
        match k {
            "name" => v.clone_into(&mut settings.name),
            "map" => v.clone_into(&mut settings.map),
            "size" => size = Some(two(v).map_err(at)?),
            "origin" => origin = Some(two(v).map_err(at)?),
            "start" => {
                let f = v
                    .split_whitespace()
                    .map(|x| number::<f64>(x, "start"))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(at)?;
                let [x, y, facing] = f[..] else {
                    return Err(at("start x y facing".into()));
                };
                settings.start = Some(Start {
                    x: centi(x),
                    y: centi(y),
                    facing: centi(facing),
                });
            }
            "borrow" => {
                let (id, name) = v
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| at("borrow id name".into()))?;
                settings.borrow = Some(Borrow {
                    area: number(id, "borrow").map_err(at)?,
                    name: name.trim().to_owned(),
                });
            }
            "author" => {
                let (name, made) = v
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| at("author name count".into()))?;
                check_author(name).map_err(at)?;
                authors.push(Author {
                    name: name.to_owned(),
                    made: number(made, "an author's count").map_err(at)?,
                });
            }
            _ => return Err(at(format!("no key {k}"))),
        }
    }
    let (Some(size), Some(origin)) = (size, origin) else {
        return Err("no size and origin".into());
    };
    let fits = |o: u32, s: u32| {
        (1..=MAX_ZONE_TILES).contains(&s) && o.checked_add(s).is_some_and(|e| e <= MAP_TILES)
    };
    if !fits(origin.0, size.0) || !fits(origin.1, size.1) {
        return Err("a size or origin off the map".into());
    }
    if settings.map.is_empty() || settings.name.is_empty() {
        return Err("no name or map".into());
    }
    Ok((Frame { origin, size }, settings, authors))
}

fn heights_bytes(h: &Heights) -> Vec<u8> {
    let mut b = Vec::with_capacity(12 + 4 * h.len());
    b.extend_from_slice(HEIGHTS_MAGIC);
    b.extend_from_slice(&(h.cols as u32).to_le_bytes());
    b.extend_from_slice(&(h.rows as u32).to_le_bytes());
    for v in h.outer.iter().chain(&h.inner) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

fn read_heights(b: &[u8], cols: usize, rows: usize) -> Result<Heights, String> {
    if b.get(..4) != Some(HEIGHTS_MAGIC) {
        return Err("not a zone's heights".into());
    }
    let u = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    if (u(4), u(8)) != (Some(cols as u32), Some(rows as u32)) {
        return Err("its size differs from zone.txt's".into());
    }
    let (outer, inner) = ((cols + 1) * (rows + 1), cols * rows);
    if b.len() != 12 + 4 * (outer + inner) {
        return Err("cut short or overlong".into());
    }
    let values: Vec<f32> = b[12..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| f32::from_le_bytes(*s))
        .collect();
    if values.iter().any(|v| !v.is_finite()) {
        return Err("a height that is not a number".into());
    }
    Ok(Heights {
        cols,
        rows,
        inner: values[outer..].to_vec(),
        outer: values[..outer].to_vec(),
    })
}

fn paint_bytes(p: &[ChunkPaint]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(PAINT_MAGIC);
    b.extend_from_slice(&(p.len() as u32).to_le_bytes());
    for c in p {
        b.push(c.layers.len() as u8);
        for l in &c.layers {
            b.extend_from_slice(&l.palette_place.to_le_bytes());
            b.extend_from_slice(&l.w);
        }
    }
    b
}

fn read_paint(b: &[u8], chunks: usize, textures: usize) -> Result<Vec<ChunkPaint>, String> {
    if b.get(..4) != Some(PAINT_MAGIC) {
        return Err("not a zone's paint".into());
    }
    if b.get(4..8)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        != Some(chunks as u32)
    {
        return Err("its chunk count differs from zone.txt's".into());
    }
    let mut at = 8;
    let mut out = Vec::with_capacity(chunks);
    for _ in 0..chunks {
        let n = usize::from(*b.get(at).ok_or("cut short")?);
        at += 1;
        if !(1..=MAX_LAYERS).contains(&n) {
            return Err(format!("a chunk of {n} textures"));
        }
        let mut layers = Vec::with_capacity(n);
        for _ in 0..n {
            let s = b.get(at..at + 2 + TEXELS_IN_CHUNK).ok_or("cut short")?;
            let tex = u16::from_le_bytes([s[0], s[1]]);
            if usize::from(tex) >= textures
                || layers.iter().any(|l: &PaintLayer| l.palette_place == tex)
            {
                return Err(format!(
                    "a chunk paints with texture {tex}, not the palette's"
                ));
            }
            layers.push(PaintLayer {
                palette_place: tex,
                w: s[2..].to_vec(),
            });
            at += 2 + TEXELS_IN_CHUNK;
        }
        out.push(ChunkPaint { layers });
    }
    if at != b.len() {
        return Err("bytes after its last chunk".into());
    }
    Ok(out)
}

fn palette_text(p: &[Texture]) -> String {
    let mut s =
        String::from("# place effect texture (effect: its GroundEffectTexture id, - for none)\n");
    for (i, t) in p.iter().enumerate() {
        let effect = if t.effect == 0 {
            "-".to_owned()
        } else {
            t.effect.to_string()
        };
        let _ = writeln!(s, "{i} {effect} {}", t.path);
    }
    s
}

fn parse_palette(s: &str) -> Result<Vec<Texture>, String> {
    let mut out = Vec::new();
    for (n, l) in lines(s) {
        let mut f = l.splitn(3, char::is_whitespace);
        let (Some(i), Some(e), Some(p)) = (f.next(), f.next(), f.next()) else {
            return Err(format!("line {n}: want `place effect texture`"));
        };
        if number::<usize>(i, "a place")? != out.len() {
            return Err(format!("line {n}: out of order"));
        }
        out.push(Texture {
            path: p.trim().to_owned(),
            effect: if e == "-" { 0 } else { number(e, "an effect")? },
        });
    }
    if out.is_empty() {
        return Err("no texture".into());
    }
    Ok(out)
}
