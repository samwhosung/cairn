pub mod alpha;
mod liquid;
mod normals;
mod place;
pub mod tile;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::frame::{CENTRE, MAP_TILES, TILE, heading_of};
use crate::install::{Install, ModelBox};
use crate::text::two_places;
use crate::zone::{Start, Thing, Zone};
use place::{Cover, Doodad, Wmo, doodad_tables, rotation_on_ground, wmo_tables};

pub use liquid::wets;

/// The textures one tile can name: the client's table has no room for more.
const MAX_TILE_TEXTURES: usize = 99;

/// What a zone writes: every tile's bytes by its name's `(x, y)`, the WDT, the WDL and cairn's
/// `zone.txt`.
pub struct Built {
    pub tiles: BTreeMap<(u32, u32), Vec<u8>>,
    pub wdt: Vec<u8>,
    pub wdl: Vec<u8>,
    pub zone_file: String,
}

enum Record {
    Doodad(Doodad),
    Wmo(Wmo),
}

/// The name `MMDX` gives a model: the shipped tiles name `.mdx`.
pub fn mmdx_name(model: &str) -> String {
    let l = model.to_ascii_lowercase();
    match l.strip_suffix(".m2").or_else(|| l.strip_suffix(".mdl")) {
        Some(s) => format!("{}.mdx", &model[..s.len()]),
        None => model.to_owned(),
    }
}

fn record(z: &Zone, unique_id: u32, t: &Thing, b: ModelBox) -> Record {
    let o = z.frame.origin;
    let p = t.at();
    let position = [
        (f64::from(o.0) * TILE + p[0]) as f32,
        t.world_z(&z.heights) as f32,
        (f64::from(o.1) * TILE + p[1]) as f32,
    ];
    let heading = heading_of(t.facing_deg());
    let rotation = if t.lean {
        rotation_on_ground(z.heights.rise(p), heading)
    } else {
        [0.0, heading as f32, 0.0]
    };
    match t.set {
        Some(set) => Record::Wmo(Wmo {
            model: t.model.clone(),
            unique_id,
            position,
            rotation,
            bounds: Wmo::bounds_of(position, rotation, &b),
            doodad_set: set,
        }),
        None => Record::Doodad(Doodad {
            mmdx_name: mmdx_name(&t.model),
            unique_id,
            position,
            rotation,
            scale: t.scale,
            model_box: b,
        }),
    }
}

fn meets_tile(tile: (u32, u32), c: &Cover) -> bool {
    let (north, west) = (
        CENTRE - f64::from(tile.1) * TILE,
        CENTRE - f64::from(tile.0) * TILE,
    );
    c.x[0] <= north && c.x[1] >= north - TILE && c.y[0] <= west && c.y[1] >= west - TILE
}

fn tile_heights(z: &Zone) -> BTreeMap<(u32, u32), normals::TileHeights> {
    let h = &z.heights;
    let f = z.frame;
    let mut out = BTreeMap::new();
    for (tx, ty) in (0..f.size.1).flat_map(|ty| (0..f.size.0).map(move |tx| (tx, ty))) {
        let (mut base, mut mcvt) = (Vec::with_capacity(256), Vec::with_capacity(256));
        for e in 0..256 {
            let (gx, gy) = (tx as usize * 16 + e % 16, ty as usize * 16 + e / 16);
            let (i0, j0) = (gx * 8, gy * 8);
            let b = h.outer(i0, j0);
            base.push(b);
            mcvt.push(std::array::from_fn(|i| {
                let (r, c) = (i / 17, i % 17);
                if c < 9 {
                    h.outer(i0 + c, j0 + r) - b
                } else {
                    h.inner(i0 + c - 9, j0 + r) - b
                }
            }));
        }
        out.insert(
            (f.origin.0 + tx, f.origin.1 + ty),
            normals::TileHeights { base, mcvt },
        );
    }
    out
}

pub fn build(z: &Zone, install: &mut dyn Install) -> Result<Built, String> {
    let mut records = Vec::with_capacity(z.things.len());
    for (id, t) in &z.things {
        let unique = z
            .unique_id(id)
            .ok_or_else(|| format!("{id}: its author is not among the zone's"))?;
        records.push(record(z, unique, t, install.model_box(&t.model)?));
    }
    let heights = tile_heights(z);
    let mut tiles = BTreeMap::new();
    for key in z.frame.tiles() {
        tiles.insert(key, one_tile(z, key, &heights, &records)?);
    }
    Ok(Built {
        wdt: wdt(&tiles.keys().copied().collect::<Vec<_>>()),
        wdl: wdl(z),
        zone_file: zone_file(z),
        tiles,
    })
}

fn field(
    key: (u32, u32),
    heights: &BTreeMap<(u32, u32), normals::TileHeights>,
) -> normals::Field<'_> {
    let mut field = normals::Field::alone(&heights[&key]);
    for dy in -1..=1isize {
        for dx in -1..=1isize {
            let x = key.0.checked_add_signed(dx as i32);
            let y = key.1.checked_add_signed(dy as i32);
            if let (Some(x), Some(y)) = (x, y)
                && (dx, dy) != (0, 0)
            {
                field.set_neighbour(dx, dy, heights.get(&(x, y)));
            }
        }
    }
    field
}

/// The textures a tile's chunks paint with, in palette order.
struct TileTextures<'a> {
    paths: Vec<&'a str>,
    mtex_by_palette_place: Vec<u32>,
}

fn textures(z: &Zone, (tx, ty): (usize, usize)) -> TileTextures<'_> {
    let east = z.chunks_east();
    let mut used = vec![false; z.palette.len()];
    for e in 0..256 {
        let (gx, gy) = (tx * 16 + e % 16, ty * 16 + e / 16);
        for l in &z.paint[gy * east + gx].layers {
            used[usize::from(l.palette_place)] = true;
        }
    }
    let mut t = TileTextures {
        paths: Vec::new(),
        mtex_by_palette_place: vec![0; z.palette.len()],
    };
    for (i, texture) in z.palette.iter().enumerate().filter(|(i, _)| used[*i]) {
        t.mtex_by_palette_place[i] = t.paths.len() as u32;
        t.paths.push(texture.path.as_str());
    }
    t
}

fn one_tile(
    z: &Zone,
    key: (u32, u32),
    heights: &BTreeMap<(u32, u32), normals::TileHeights>,
    records: &[Record],
) -> Result<Vec<u8>, String> {
    let (tx, ty) = (
        (key.0 - z.frame.origin.0) as usize,
        (key.1 - z.frame.origin.1) as usize,
    );
    let own = &heights[&key];
    let field = field(key, heights);
    let (doodads, wmos): (Vec<&Doodad>, Vec<&Wmo>) = (
        records
            .iter()
            .filter_map(|r| match r {
                Record::Doodad(d) if meets_tile(key, &d.cover()) => Some(d),
                _ => None,
            })
            .collect(),
        records
            .iter()
            .filter_map(|r| match r {
                Record::Wmo(w) if meets_tile(key, &w.cover()) => Some(w),
                _ => None,
            })
            .collect(),
    );
    let doodad_covers: Vec<Cover> = doodads.iter().map(|d| d.cover()).collect();
    let wmo_covers: Vec<Cover> = wmos.iter().map(|w| w.cover()).collect();
    let east = z.chunks_east();
    let textures = textures(z, (tx, ty));
    if textures.paths.len() > MAX_TILE_TEXTURES {
        return Err(format!(
            "tile {},{} paints with {} textures; a tile can name {MAX_TILE_TEXTURES}",
            key.0,
            key.1,
            textures.paths.len()
        ));
    }
    let area = z.settings.borrow.as_ref().map_or(0, |b| b.area);
    let mut chunks = Vec::with_capacity(256);
    for e in 0..256 {
        let (gx, gy) = (tx * 16 + e % 16, ty * 16 + e / 16);
        let paint = &z.paint[gy * east + gx];
        let weights: Vec<&[u8]> = paint.layers.iter().map(|l| l.w.as_slice()).collect();
        let nibbles: Vec<alpha::AlphaMap> = alpha::alpha_maps(&weights)
            .iter()
            .map(alpha::nibbles)
            .collect();
        let (north, west) = tile::chunk_corner(key, e / 16, e % 16);
        let refs = |covers: &[Cover]| -> Vec<u32> {
            (0..covers.len() as u32)
                .filter(|&k| covers[k as usize].meets_chunk(f64::from(north), f64::from(west)))
                .collect()
        };
        chunks.push(tile::Chunk {
            base: own.base[e],
            mcvt: own.mcvt[e],
            normals: std::array::from_fn(|i| normals::bake(&field, e, i)),
            layers: paint
                .layers
                .iter()
                .map(|l| tile::Layer {
                    mtex_index: textures.mtex_by_palette_place[usize::from(l.palette_place)],
                    effect: z.palette[usize::from(l.palette_place)].effect,
                })
                .collect(),
            alpha: nibbles.iter().map(alpha::pack).collect(),
            predominant: alpha::predominant(&nibbles),
            water: liquid::chunk_water(z, gx, gy),
            area,
            doodad_refs: refs(&doodad_covers),
            wmo_refs: refs(&wmo_covers),
        });
    }
    let doodads: Vec<Doodad> = doodads.into_iter().cloned().collect();
    let wmos: Vec<Wmo> = wmos.into_iter().cloned().collect();
    Ok(tile::tile(
        key,
        &textures.paths,
        &doodad_tables(&doodads),
        &wmo_tables(&wmos),
        &chunks,
    ))
}

fn chunk_record(magic: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&magic);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// A terrain map's WDT still carries an empty `MWMO`.
fn wdt(tiles: &[(u32, u32)]) -> Vec<u8> {
    let mut main = vec![0u8; (MAP_TILES * MAP_TILES) as usize * 8];
    for &(x, y) in tiles {
        main[(y * MAP_TILES + x) as usize * 8] = 1;
    }
    let mut out = chunk_record(*b"REVM", &18u32.to_le_bytes());
    out.extend(chunk_record(*b"DHPM", &[0; 32]));
    out.extend(chunk_record(*b"NIAM", &main));
    out.extend(chunk_record(*b"OMWM", &[]));
    out
}

/// Cut toward zero, as the shipped heights are.
fn wdl_height(v: f32) -> i16 {
    v.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

/// The map's WDL: `MVER`, `MAOF` with each tile's offset, and a `MARE` a tile, the heights at
/// its chunks' corners (17×17) and middles (16×16).
fn wdl(z: &Zone) -> Vec<u8> {
    let h = &z.heights;
    let mut out = chunk_record(*b"REVM", &18u32.to_le_bytes());
    let maof = out.len() + 8;
    out.extend(chunk_record(
        *b"FOAM",
        &[0; (MAP_TILES * MAP_TILES) as usize * 4],
    ));
    for (x, y) in z.frame.tiles() {
        let (tx, ty) = (
            (x - z.frame.origin.0) as usize * 128,
            (y - z.frame.origin.1) as usize * 128,
        );
        let corners = (0..17).flat_map(|r| (0..17).map(move |c| (tx + c * 8, ty + r * 8)));
        let middles = (0..16).flat_map(|r| (0..16).map(move |c| (tx + c * 8 + 4, ty + r * 8 + 4)));
        let mare: Vec<u8> = corners
            .chain(middles)
            .flat_map(|(i, j)| wdl_height(h.outer(i, j)).to_le_bytes())
            .collect();
        let at = out.len() as u32;
        let entry = maof + (y * MAP_TILES + x) as usize * 4;
        out[entry..entry + 4].copy_from_slice(&at.to_le_bytes());
        out.extend(chunk_record(*b"ERAM", &mare));
    }
    out
}

/// Degrees from north toward west, as cairn's `zone.txt` gives a facing, of a compass bearing.
fn toward_west(bearing: f64) -> f64 {
    (360.0 - bearing).rem_euclid(360.0)
}

fn zone_file(z: &Zone) -> String {
    let e = z.frame.extent();
    let st = z.settings.start.unwrap_or(Start {
        x: crate::text::centi(e[0] / 2.0),
        y: crate::text::centi(e[1] / 2.0),
        facing: 0,
    });
    let p = [st.x as f64 / 100.0, st.y as f64 / 100.0];
    let w = z.frame.world(p);
    let mut s = format!(
        "# written by `cairn zone` from {}, whose own frame is x east, y south of its north-west \
         corner\nname = {}\nstart = {:.2}, {:.2}, {:.2}\nfacing = {}\n",
        z.settings.name,
        z.settings.map,
        w[0],
        w[1],
        z.heights.ground(p),
        two_places(toward_west(st.facing as f64 / 100.0))
    );
    if let Some(b) = &z.settings.borrow {
        let _ = writeln!(s, "borrows = {}", b.name);
    }
    s
}

/// Write a build into `dir` in the install's layout: `World/Maps/<map>/`, and `zone.txt`.
/// Whatever else `dir/World` held goes.
pub fn write(b: &Built, dir: &Path, map: &str) -> Result<(), String> {
    let io = |e: std::io::Error| format!("{}: {e}", dir.display());
    let world = dir.join("World");
    if world.exists() {
        std::fs::remove_dir_all(&world).map_err(io)?;
    }
    let maps = world.join("Maps").join(map);
    std::fs::create_dir_all(&maps).map_err(io)?;
    std::fs::write(maps.join(format!("{map}.wdt")), &b.wdt).map_err(io)?;
    std::fs::write(maps.join(format!("{map}.wdl")), &b.wdl).map_err(io)?;
    for ((x, y), bytes) in &b.tiles {
        std::fs::write(maps.join(format!("{map}_{x}_{y}.adt")), bytes).map_err(io)?;
    }
    std::fs::write(dir.join("zone.txt"), &b.zone_file).map_err(io)
}
