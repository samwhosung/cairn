use std::collections::BTreeSet;

use crate::frame::CHUNK;
use crate::text::{parse_id, parse_thing, parse_water, thing_text, water_text};
use crate::zone::{
    Borrow, ChunkPaint, Id, PaintLayer, Settings, Start, TEXELS_IN_CHUNK, Thing, Water, Zone,
};

/// Heights of consecutive vertices, from `start` in [`crate::zone::Heights::get`]'s order.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightRun {
    pub start: u32,
    pub values: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexelRun {
    pub first: u16,
    pub count: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayerImage {
    pub palette_place: u16,
    /// Its weights on the image's texels, run after run.
    pub weights: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChunkImage {
    pub chunk: u32,
    pub texels: Vec<TexelRun>,
    /// The chunk's textures, in order.
    pub layers: Vec<LayerImage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effect {
    pub place: u16,
    pub effect: u32,
}

/// What a command's footprint holds at one moment: the heights of the vertices it moved, the
/// weights of the texels it changed with their chunks' texture lists, and the things, water,
/// ground effects and settings it touched.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Image {
    pub heights: Vec<HeightRun>,
    pub paint: Vec<ChunkImage>,
    pub effects: Vec<Effect>,
    pub things: Vec<(Id, Option<Thing>)>,
    pub water: Vec<(Id, Option<Water>)>,
    pub settings: Option<Settings>,
}

/// What an image covers: chunks as `(east, south)` in the zone's grid of chunks, and ground
/// effects by their texture's place in the palette.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Footprint {
    pub chunks: BTreeSet<(usize, usize)>,
    pub things: BTreeSet<Id>,
    pub water: BTreeSet<Id>,
    pub effects: BTreeSet<u16>,
    pub settings: bool,
}

impl Footprint {
    pub fn add(&mut self, other: Footprint) {
        self.chunks.extend(other.chunks);
        self.things.extend(other.things);
        self.water.extend(other.water);
        self.effects.extend(other.effects);
        self.settings |= other.settings;
    }

    pub fn meets(&self, other: &Footprint) -> bool {
        !self.chunks.is_disjoint(&other.chunks)
            || !self.things.is_disjoint(&other.things)
            || !self.water.is_disjoint(&other.water)
            || !self.effects.is_disjoint(&other.effects)
            || (self.settings && other.settings)
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        let chunks: Vec<String> = self
            .chunks
            .iter()
            .map(|(x, y)| format!("{x},{y}"))
            .collect();
        let list = |v: &[String]| {
            if v.len() > 6 {
                format!("{} and {} more", v[..6].join(" "), v.len() - 6)
            } else {
                v.join(" ")
            }
        };
        if !chunks.is_empty() {
            parts.push(format!("chunk {}", list(&chunks)));
        }
        let things: Vec<String> = self.things.iter().map(ToString::to_string).collect();
        if !things.is_empty() {
            parts.push(format!("thing {}", list(&things)));
        }
        let water: Vec<String> = self.water.iter().map(ToString::to_string).collect();
        if !water.is_empty() {
            parts.push(format!("water {}", list(&water)));
        }
        if !self.effects.is_empty() {
            parts.push("a texture's ground effect".into());
        }
        if self.settings {
            parts.push("the zone's settings".into());
        }
        if parts.is_empty() {
            "nothing".into()
        } else {
            parts.join(", ")
        }
    }
}

impl Image {
    /// Heights of vertices, from `(vertex, height)` in vertex order.
    pub fn heights_of(pairs: impl IntoIterator<Item = (usize, f32)>) -> Vec<HeightRun> {
        let mut runs: Vec<HeightRun> = Vec::new();
        for (k, v) in pairs {
            match runs.last_mut() {
                Some(r) if r.start as usize + r.values.len() == k => r.values.push(v),
                _ => runs.push(HeightRun {
                    start: k as u32,
                    values: vec![v],
                }),
            }
        }
        runs
    }

    /// A chunk's paint as it was, on the texels where it differs from `now`.
    pub fn changed_paint(chunk: usize, was: &ChunkPaint, now: &ChunkPaint) -> Option<ChunkImage> {
        let weight = |p: &ChunkPaint, tex: u16, t: usize| {
            p.layers
                .iter()
                .find(|l| l.palette_place == tex)
                .map_or(0, |l| l.w[t])
        };
        let texs: BTreeSet<u16> = was
            .layers
            .iter()
            .chain(&now.layers)
            .map(|l| l.palette_place)
            .collect();
        let differs = |t: usize| texs.iter().any(|&x| weight(was, x, t) != weight(now, x, t));
        let texels = runs_of_sorted((0..TEXELS_IN_CHUNK).filter(|&t| differs(t)));
        let same_list = was
            .layers
            .iter()
            .map(|l| l.palette_place)
            .eq(now.layers.iter().map(|l| l.palette_place));
        if texels.is_empty() && same_list {
            return None;
        }
        Some(ChunkImage {
            chunk: chunk as u32,
            layers: take(was, &texels),
            texels,
        })
    }

    /// The same footprint as it is in `zone` now.
    pub fn capture(&self, zone: &Zone) -> Image {
        Image {
            heights: self
                .heights
                .iter()
                .map(|r| HeightRun {
                    start: r.start,
                    values: (0..r.values.len())
                        .map(|i| zone.heights.get(r.start as usize + i))
                        .collect(),
                })
                .collect(),
            paint: self
                .paint
                .iter()
                .map(|c| ChunkImage {
                    chunk: c.chunk,
                    texels: c.texels.clone(),
                    layers: take(&zone.paint[c.chunk as usize], &c.texels),
                })
                .collect(),
            effects: self
                .effects
                .iter()
                .map(|e| Effect {
                    place: e.place,
                    effect: zone.palette[usize::from(e.place)].effect,
                })
                .collect(),
            things: self
                .things
                .iter()
                .map(|(id, _)| (id.clone(), zone.things.get(id).cloned()))
                .collect(),
            water: self
                .water
                .iter()
                .map(|(id, _)| (id.clone(), zone.water.get(id).cloned()))
                .collect(),
            settings: self.settings.as_ref().map(|_| zone.settings.clone()),
        }
    }

    pub fn restore(&self, zone: &mut Zone) {
        for r in &self.heights {
            for (i, &v) in r.values.iter().enumerate() {
                zone.heights.set(r.start as usize + i, v);
            }
        }
        for c in &self.paint {
            let chunk = &mut zone.paint[c.chunk as usize];
            let layers = c
                .layers
                .iter()
                .map(|image| {
                    let mut w = chunk
                        .layers
                        .iter()
                        .find(|l| l.palette_place == image.palette_place)
                        .map_or_else(|| vec![0; TEXELS_IN_CHUNK], |l| l.w.clone());
                    for (t, &v) in texels(&c.texels).zip(&image.weights) {
                        w[t] = v;
                    }
                    PaintLayer {
                        palette_place: image.palette_place,
                        w,
                    }
                })
                .collect();
            chunk.layers = layers;
        }
        for e in &self.effects {
            zone.palette[usize::from(e.place)].effect = e.effect;
        }
        for (id, t) in &self.things {
            match t {
                Some(t) => {
                    #[cfg(test)]
                    let t = &control::restored(zone.things.get(id), t);
                    zone.things.insert(id.clone(), t.clone())
                }
                None => zone.things.remove(id),
            };
        }
        for (id, w) in &self.water {
            match w {
                Some(w) => zone.water.insert(id.clone(), w.clone()),
                None => zone.water.remove(id),
            };
        }
        if let Some(s) = &self.settings {
            zone.settings = s.clone();
        }
    }

    pub fn places_exist_in(&self, zone: &Zone) -> Result<(), String> {
        let vertices = zone.heights.len();
        if let Some(r) = self
            .heights
            .iter()
            .find(|r| r.start as usize + r.values.len() > vertices)
        {
            return Err(format!(
                "a height run from vertex {} is off the zone",
                r.start
            ));
        }
        for c in &self.paint {
            let chunk_known = (c.chunk as usize) < zone.paint.len();
            let sum: usize = c.texels.iter().map(|r| usize::from(r.count)).sum();
            let bad = !chunk_known
                || c.texels
                    .iter()
                    .any(|r| usize::from(r.first) + usize::from(r.count) > TEXELS_IN_CHUNK)
                || c.layers.iter().any(|l| {
                    usize::from(l.palette_place) >= zone.palette.len() || l.weights.len() != sum
                })
                || c.layers.is_empty();
            if bad {
                return Err(format!(
                    "the paint of chunk {} does not fit the zone",
                    c.chunk
                ));
            }
        }
        if self
            .effects
            .iter()
            .any(|e| usize::from(e.place) >= zone.palette.len())
        {
            return Err("a ground effect names a texture the palette lacks".into());
        }
        Ok(())
    }

    pub fn footprint(&self, zone: &Zone) -> Footprint {
        let mut f = Footprint::default();
        let (east, south) = zone.frame.chunks();
        let chunk_of = |p: f64, n: usize| ((p / CHUNK).floor().max(0.0) as usize).min(n - 1);
        for r in &self.heights {
            for i in 0..r.values.len() {
                let p = zone.heights.point(r.start as usize + i);
                f.chunks
                    .insert((chunk_of(p[0], east), chunk_of(p[1], south)));
            }
        }
        for c in &self.paint {
            let c = c.chunk as usize;
            f.chunks.insert((c % east, c / east));
        }
        f.things = self.things.iter().map(|(id, _)| id.clone()).collect();
        f.water = self.water.iter().map(|(id, _)| id.clone()).collect();
        f.effects = self.effects.iter().map(|e| e.place).collect();
        f.settings = self.settings.is_some();
        f
    }

    pub fn digest(&self) -> u64 {
        let mut b = Vec::new();
        self.encode(&mut b);
        crate::choice::hash_bytes(&b)
    }

    pub fn encode(&self, b: &mut Vec<u8>) {
        put_u32(b, self.heights.len());
        for r in &self.heights {
            b.extend_from_slice(&r.start.to_le_bytes());
            put_u32(b, r.values.len());
            for v in &r.values {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        put_u32(b, self.paint.len());
        for c in &self.paint {
            b.extend_from_slice(&c.chunk.to_le_bytes());
            put_u32(b, c.texels.len());
            for r in &c.texels {
                b.extend_from_slice(&r.first.to_le_bytes());
                b.extend_from_slice(&r.count.to_le_bytes());
            }
            b.push(c.layers.len() as u8);
            for l in &c.layers {
                b.extend_from_slice(&l.palette_place.to_le_bytes());
                b.extend_from_slice(&l.weights);
            }
        }
        put_u32(b, self.effects.len());
        for e in &self.effects {
            b.extend_from_slice(&e.place.to_le_bytes());
            b.extend_from_slice(&e.effect.to_le_bytes());
        }
        put_u32(b, self.things.len());
        for (id, t) in &self.things {
            put_str(b, &id.to_string());
            put_opt(b, t.as_ref().map(thing_text));
        }
        put_u32(b, self.water.len());
        for (id, w) in &self.water {
            put_str(b, &id.to_string());
            put_opt(b, w.as_ref().map(water_text));
        }
        put_opt(b, self.settings.as_ref().map(settings_text));
    }

    pub fn decode(bytes: &[u8]) -> Result<Image, String> {
        let mut r = Reader { b: bytes, at: 0 };
        let mut image = Image::default();
        for _ in 0..r.count()? {
            let start = r.u32()?;
            let n = r.count()?;
            let values = (0..n)
                .map(|_| r.u32().map(f32::from_bits))
                .collect::<Result<_, _>>()?;
            image.heights.push(HeightRun { start, values });
        }
        for _ in 0..r.count()? {
            let chunk = r.u32()?;
            let texels: Vec<TexelRun> = (0..r.count()?)
                .map(|_| {
                    Ok(TexelRun {
                        first: r.u16()?,
                        count: r.u16()?,
                    })
                })
                .collect::<Result<_, String>>()?;
            let sum: usize = texels.iter().map(|t| usize::from(t.count)).sum();
            let layers = (0..r.u8()?)
                .map(|_| {
                    Ok(LayerImage {
                        palette_place: r.u16()?,
                        weights: r.bytes(sum)?.to_vec(),
                    })
                })
                .collect::<Result<_, String>>()?;
            image.paint.push(ChunkImage {
                chunk,
                texels,
                layers,
            });
        }
        for _ in 0..r.count()? {
            image.effects.push(Effect {
                place: r.u16()?,
                effect: r.u32()?,
            });
        }
        for _ in 0..r.count()? {
            let id = parse_id(&r.str()?)?;
            let t = r.opt()?.map(|s| parse_thing(&s)).transpose()?;
            image.things.push((id, t));
        }
        for _ in 0..r.count()? {
            let id = parse_id(&r.str()?)?;
            let w = r.opt()?.map(|s| parse_water(&s)).transpose()?;
            image.water.push((id, w));
        }
        image.settings = r.opt()?.map(|s| parse_settings(&s)).transpose()?;
        if r.at != bytes.len() {
            return Err("an image with bytes left over".into());
        }
        Ok(image)
    }
}

fn runs_of_sorted(ks: impl IntoIterator<Item = usize>) -> Vec<TexelRun> {
    let mut out: Vec<TexelRun> = Vec::new();
    for k in ks {
        match out.last_mut() {
            Some(r) if usize::from(r.first) + usize::from(r.count) == k => r.count += 1,
            _ => out.push(TexelRun {
                first: k as u16,
                count: 1,
            }),
        }
    }
    out
}

fn texels(runs: &[TexelRun]) -> impl Iterator<Item = usize> + '_ {
    runs.iter()
        .flat_map(|r| usize::from(r.first)..usize::from(r.first) + usize::from(r.count))
}

fn take(p: &ChunkPaint, runs: &[TexelRun]) -> Vec<LayerImage> {
    p.layers
        .iter()
        .map(|l| LayerImage {
            palette_place: l.palette_place,
            weights: texels(runs).map(|t| l.w[t]).collect(),
        })
        .collect()
}

fn settings_text(s: &Settings) -> String {
    let start = s.start.map_or("-".to_owned(), |st| {
        format!("{} {} {}", st.x, st.y, st.facing)
    });
    let borrow = s
        .borrow
        .as_ref()
        .map_or("-".to_owned(), |b| format!("{} {}", b.area, b.name));
    format!("{}\n{}\n{start}\n{borrow}", s.name, s.map)
}

fn parse_settings(s: &str) -> Result<Settings, String> {
    let bad = || "settings that don't read".to_owned();
    let mut l = s.split('\n');
    let (name, map, start, borrow) = (
        l.next().ok_or_else(bad)?,
        l.next().ok_or_else(bad)?,
        l.next().ok_or_else(bad)?,
        l.next().ok_or_else(bad)?,
    );
    let start = match start {
        "-" => None,
        s => {
            let v = s
                .split(' ')
                .map(|n| n.parse::<i64>().map_err(|_| bad()))
                .collect::<Result<Vec<_>, _>>()?;
            let [x, y, facing] = v[..] else {
                return Err(bad());
            };
            Some(Start { x, y, facing })
        }
    };
    let borrow = match borrow {
        "-" => None,
        s => {
            let (id, n) = s.split_once(' ').ok_or_else(bad)?;
            Some(Borrow {
                area: id.parse().map_err(|_| bad())?,
                name: n.to_owned(),
            })
        }
    };
    Ok(Settings {
        name: name.to_owned(),
        map: map.to_owned(),
        start,
        borrow,
    })
}

fn put_u32(b: &mut Vec<u8>, n: usize) {
    b.extend_from_slice(&(n as u32).to_le_bytes());
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    put_u32(b, s.len());
    b.extend_from_slice(s.as_bytes());
}

fn put_opt(b: &mut Vec<u8>, s: Option<String>) {
    match s {
        Some(s) => {
            b.push(1);
            put_str(b, &s);
        }
        None => b.push(0),
    }
}

/// Bounds-checked reads over a record, each failing with an error rather than a panic.
pub struct Reader<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl Reader<'_> {
    pub fn bytes(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len());
        let end = end.ok_or("a record cut short")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        let s = self.bytes(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let s = self.bytes(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from(self.u32()?) | u64::from(self.u32()?) << 32)
    }

    /// A count of things still to read, each at least a byte: more than the bytes left is a lie.
    pub fn count(&mut self) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > self.b.len() - self.at {
            return Err("a count larger than the record".into());
        }
        Ok(n)
    }

    pub fn str(&mut self) -> Result<String, String> {
        let n = self.count()?;
        String::from_utf8(self.bytes(n)?.to_vec()).map_err(|_| "text that is not UTF-8".into())
    }

    pub fn opt(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.str().map(Some),
            _ => Err("a flag that is neither 0 nor 1".into()),
        }
    }
}

#[cfg(test)]
pub mod control {
    use std::cell::Cell;

    use crate::zone::Thing;

    thread_local!(static DROP_FACING: Cell<bool> = const { Cell::new(false) });

    /// The undo check's control, on this thread: an image restores a thing without its facing,
    /// which stays as it finds it.
    pub fn drop_facing() {
        DROP_FACING.with(|d| d.set(true));
    }

    pub fn restored(now: Option<&Thing>, t: &Thing) -> Thing {
        let mut t = t.clone();
        if let Some(now) = now.filter(|_| DROP_FACING.with(Cell::get)) {
            t.facing = now.facing;
        }
        t
    }
}
