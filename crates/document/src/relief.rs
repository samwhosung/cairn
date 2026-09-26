//! A zone's relief, as the relief brush lays it: the install's ground less its own smoothed shape.

use std::collections::BTreeMap;
use std::io::Cursor;

use dbc::{DbcParser, FieldType, Record, Schema, SchemaField, Value};
use mpq::Chain;

use crate::frame::{CENTRE, TILE};
use crate::install::Areas;
use crate::zone::Heights;

/// A patch's side, in cells.
pub const PATCH: usize = 16;
/// How far two patches overlap, in cells.
pub const OVERLAP: usize = 4;
const SOURCE_STRIDE: usize = 2;
const CELLS_A_CHUNK: usize = 8;
const BLUR_SIGMA: f64 = CELLS_A_CHUNK as f64;
const MAP_DBC: &str = "DBFilesClient\\Map.dbc";
const MAP_COLUMNS: usize = 42;
const WORLD_MAP_AREA: &str = "DBFilesClient\\WorldMapArea.dbc";
const CHUNKS_A_TILE: u32 = 16;
const MCVT_ROW: usize = 17;
const MCVT_HEIGHTS: usize = 145;

/// Heights over a box of cells, and which of the cells have ground.
#[derive(Clone, Debug, PartialEq)]
pub struct Ground {
    pub heights: Heights,
    pub known: Vec<bool>,
}

/// A zone's relief, 0 off its ground, and where the patches the brush copies start: squares of
/// [`PATCH`] cells, each all the zone's, the smoothest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub zone: String,
    pub relief: Heights,
    pub patches: Vec<(u32, u32)>,
    /// The relief's spread over the zone's ground, its root mean square, in yards.
    pub spread: f64,
}

impl Source {
    pub fn of(zone: &str, g: &Ground) -> Result<Source, String> {
        let (cols, rows) = (g.heights.cols, g.heights.rows);
        let corner_known: Vec<f32> = (0..(cols + 1) * (rows + 1))
            .map(|k| {
                let (i, j) = (k % (cols + 1), k / (cols + 1));
                let near = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().any(|&(a, b)| {
                    let (ci, cj) = (i.wrapping_sub(a), j.wrapping_sub(b));
                    ci < cols && cj < rows && g.known[cj * cols + ci]
                });
                if near { 1.0 } else { 0.0 }
            })
            .collect();
        let weighted: Vec<f32> = g
            .heights
            .outer
            .iter()
            .zip(&corner_known)
            .map(|(h, k)| h * k)
            .collect();
        let (w, h) = (cols + 1, rows + 1);
        let sum = blur(&weighted, w, h);
        let weight = blur(&corner_known, w, h);
        let smooth: Vec<f32> = sum
            .iter()
            .zip(&weight)
            .map(|(s, k)| if *k > 1e-6 { s / k } else { 0.0 })
            .collect();
        let outer: Vec<f32> = (0..w * h)
            .map(|k| {
                if corner_known[k] > 0.0 {
                    g.heights.outer[k] - smooth[k]
                } else {
                    0.0
                }
            })
            .collect();
        let inner: Vec<f32> = (0..cols * rows)
            .map(|k| {
                let (i, j) = (k % cols, k / cols);
                if !g.known[k] {
                    return 0.0;
                }
                let around = smooth[j * w + i]
                    + smooth[j * w + i + 1]
                    + smooth[(j + 1) * w + i]
                    + smooth[(j + 1) * w + i + 1];
                g.heights.inner[k] - around / 4.0
            })
            .collect();
        let mut patches = patch_starts(g);
        let roughness = |&(i, j): &(u32, u32)| {
            let (i, j) = (i as usize, j as usize);
            let mut s = 0.0f64;
            for y in j..=j + PATCH {
                for x in i..=i + PATCH {
                    s += f64::from(outer[y * w + x]).powi(2);
                }
            }
            s
        };
        let mut ranked: Vec<(f64, (u32, u32))> =
            patches.iter().map(|p| (roughness(p), *p)).collect();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        patches = ranked.into_iter().map(|(_, p)| p).collect();
        if patches.is_empty() {
            return Err(format!(
                "{zone}: its ground holds no square of {PATCH} cells to copy"
            ));
        }
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for (d, k) in outer.iter().zip(&corner_known) {
            if *k > 0.0 {
                s += f64::from(*d) * f64::from(*d);
                n += 1.0;
            }
        }
        Ok(Source {
            zone: zone.to_owned(),
            relief: Heights {
                cols,
                rows,
                outer,
                inner,
            },
            patches,
            spread: (s / n.max(1.0)).sqrt(),
        })
    }
}

fn blur(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let reach = (3.0 * BLUR_SIGMA).ceil() as i64;
    let kernel: Vec<f32> = (-reach..=reach)
        .map(|d| libm::exp(-((d * d) as f64) / (2.0 * BLUR_SIGMA * BLUR_SIGMA)) as f32)
        .collect();
    let pass = |src: &[f32], along_rows: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        for j in 0..h {
            for i in 0..w {
                let mut s = 0.0f32;
                for (t, k) in kernel.iter().enumerate() {
                    let d = t as i64 - reach;
                    let (x, y) = if along_rows {
                        (i as i64 + d, j as i64)
                    } else {
                        (i as i64, j as i64 + d)
                    };
                    if (0..w as i64).contains(&x) && (0..h as i64).contains(&y) {
                        s += k * src[y as usize * w + x as usize];
                    }
                }
                out[j * w + i] = s;
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

fn patch_starts(g: &Ground) -> Vec<(u32, u32)> {
    let (cols, rows) = (g.heights.cols, g.heights.rows);
    let mut table = vec![0u32; (cols + 1) * (rows + 1)];
    for j in 0..rows {
        for i in 0..cols {
            table[(j + 1) * (cols + 1) + i + 1] = u32::from(g.known[j * cols + i])
                + table[j * (cols + 1) + i + 1]
                + table[(j + 1) * (cols + 1) + i]
                - table[j * (cols + 1) + i];
        }
    }
    let count = |i: usize, j: usize| {
        let at = |a: usize, b: usize| table[b * (cols + 1) + a];
        at(i + PATCH, j + PATCH) + at(i, j) - at(i + PATCH, j) - at(i, j + PATCH)
    };
    let mut out = Vec::new();
    for j in (0..rows.saturating_sub(PATCH - 1)).step_by(SOURCE_STRIDE) {
        for i in (0..cols.saturating_sub(PATCH - 1)).step_by(SOURCE_STRIDE) {
            if count(i, j) as usize == PATCH * PATCH {
                out.push((i as u32, j as u32));
            }
        }
    }
    out
}

/// The ground of the install's zone `name`: every chunk whose area lies in it, on the tiles its
/// world map spans and one more each way.
pub fn read(chain: &Chain, areas: &Areas, name: &str) -> Result<Ground, String> {
    let zone = areas.zone_named(name)?;
    let directory = map_directory(chain, zone.map)?;
    let bounds = world_map_edges(chain, zone.id)?
        .ok_or_else(|| format!("{name}: the install has no world map of it to find its ground"))?;
    let tile = |v: f32| ((CENTRE - f64::from(v)) / TILE).floor().clamp(0.0, 63.0) as u32;
    let (x0, x1) = (
        tile(bounds.west).saturating_sub(1),
        (tile(bounds.east) + 1).min(63),
    );
    let (y0, y1) = (
        tile(bounds.north).saturating_sub(1),
        (tile(bounds.south) + 1).min(63),
    );
    let mut chunks: BTreeMap<(u32, u32), Vec<f32>> = BTreeMap::new();
    for ty in y0..=y1 {
        for tx in x0..=x1 {
            let path = format!("World\\Maps\\{directory}\\{directory}_{tx}_{ty}.adt");
            let Ok(bytes) = chain.read(&path) else {
                continue;
            };
            let ours = |area: u32| areas.top_zone(area) == Some(zone.id);
            let found = heights_of(&bytes, (tx, ty), ours).map_err(|e| format!("{path}: {e}"))?;
            chunks.extend(found.into_iter().map(|c| (c.chunk, c.heights)));
        }
    }
    let (Some(gx0), Some(gy0)) = (
        chunks.keys().map(|k| k.0).min(),
        chunks.keys().map(|k| k.1).min(),
    ) else {
        return Err(format!("{name}: the install holds no ground of it"));
    };
    let gx1 = chunks.keys().map(|k| k.0).max().unwrap_or(gx0);
    let gy1 = chunks.keys().map(|k| k.1).max().unwrap_or(gy0);
    let cols = (gx1 - gx0 + 1) as usize * CELLS_A_CHUNK;
    let rows = (gy1 - gy0 + 1) as usize * CELLS_A_CHUNK;
    let mut g = Ground {
        heights: Heights::flat(cols, rows, 0.0),
        known: vec![false; cols * rows],
    };
    for (&(gx, gy), h) in &chunks {
        let (i0, j0) = (
            (gx - gx0) as usize * CELLS_A_CHUNK,
            (gy - gy0) as usize * CELLS_A_CHUNK,
        );
        for r in 0..=CELLS_A_CHUNK {
            for c in 0..=CELLS_A_CHUNK {
                g.heights.outer[(j0 + r) * (cols + 1) + i0 + c] = h[r * MCVT_ROW + c];
            }
        }
        for r in 0..CELLS_A_CHUNK {
            for c in 0..CELLS_A_CHUNK {
                g.heights.inner[(j0 + r) * cols + i0 + c] = h[r * MCVT_ROW + 9 + c];
                g.known[(j0 + r) * cols + i0 + c] = true;
            }
        }
    }
    Ok(g)
}

struct ChunkHeights {
    chunk: (u32, u32),
    heights: Vec<f32>,
}

fn heights_of(
    bytes: &[u8],
    (tx, ty): (u32, u32),
    ours: impl Fn(u32) -> bool,
) -> Result<Vec<ChunkHeights>, String> {
    let adt = adt::parse_adt(bytes).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for c in &adt.mcnk_chunks {
        let h = &c.header;
        let Some(heights) = c.heights.as_ref().filter(|_| ours(h.area_id)) else {
            continue;
        };
        let on_the_tile = h.index_x < CHUNKS_A_TILE && h.index_y < CHUNKS_A_TILE;
        if heights.heights.len() != MCVT_HEIGHTS || !on_the_tile {
            return Err(format!(
                "a chunk at {},{} of the tile with {} heights",
                h.index_x,
                h.index_y,
                heights.heights.len()
            ));
        }
        let base = h.position[2];
        if !base.is_finite() || heights.heights.iter().any(|v| !v.is_finite()) {
            return Err(format!(
                "a chunk at {},{} with no height",
                h.index_x, h.index_y
            ));
        }
        out.push(ChunkHeights {
            chunk: (
                tx * CHUNKS_A_TILE + h.index_x,
                ty * CHUNKS_A_TILE + h.index_y,
            ),
            heights: heights.heights.iter().map(|v| v + base).collect(),
        });
    }
    Ok(out)
}

fn map_directory(chain: &Chain, map: u32) -> Result<String, String> {
    let bytes = chain
        .read(MAP_DBC)
        .map_err(|e| format!("reading {MAP_DBC}: {e}"))?;
    let mut schema = Schema::new("Map");
    schema.add_field(SchemaField::new("id", FieldType::UInt32));
    schema.add_field(SchemaField::new("directory", FieldType::String));
    schema.add_field(SchemaField::new_array(
        "unread",
        FieldType::UInt32,
        MAP_COLUMNS - 2,
    ));
    let rows = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|e| format!("reading {MAP_DBC}: {e}"))?;
    rows.records()
        .iter()
        .find_map(|r| match (r.get_value(0), r.get_value(1)) {
            (Some(Value::UInt32(id)), Some(Value::StringRef(dir))) if *id == map => {
                rows.get_string(*dir).ok().map(std::borrow::Cow::into_owned)
            }
            _ => None,
        })
        .ok_or_else(|| format!("{MAP_DBC} has no map {map}"))
}

struct WorldMapEdges {
    west: f32,
    east: f32,
    north: f32,
    south: f32,
}

fn world_map_edges(chain: &Chain, area: u32) -> Result<Option<WorldMapEdges>, String> {
    let bytes = chain
        .read(WORLD_MAP_AREA)
        .map_err(|e| format!("reading {WORLD_MAP_AREA}: {e}"))?;
    let mut schema = Schema::new("WorldMapArea");
    for ty in [FieldType::UInt32; 3] {
        schema.add_field(SchemaField::new("", ty));
    }
    schema.add_field(SchemaField::new("", FieldType::String));
    for _ in 0..4 {
        schema.add_field(SchemaField::new("", FieldType::Float32));
    }
    let rows = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|e| format!("reading {WORLD_MAP_AREA}: {e}"))?;
    let float = |r: &Record, i: usize| match r.get_value(i) {
        Some(Value::Float32(v)) if v.is_finite() => Some(*v),
        _ => None,
    };
    Ok(rows
        .records()
        .iter()
        .filter(|r| matches!(r.get_value(2), Some(Value::UInt32(a)) if *a == area))
        .find_map(|r| {
            Some(WorldMapEdges {
                west: float(r, 4)?,
                east: float(r, 5)?,
                north: float(r, 6)?,
                south: float(r, 7)?,
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ground(cols: usize, rows: usize, f: impl Fn(f64, f64) -> f64) -> Ground {
        Ground {
            heights: Heights {
                cols,
                rows,
                outer: (0..(cols + 1) * (rows + 1))
                    .map(|k| f((k % (cols + 1)) as f64, (k / (cols + 1)) as f64) as f32)
                    .collect(),
                inner: (0..cols * rows)
                    .map(|k| f((k % cols) as f64 + 0.5, (k / cols) as f64 + 0.5) as f32)
                    .collect(),
            },
            known: vec![true; cols * rows],
        }
    }

    #[test]
    fn a_plane_has_no_relief_and_bumps_keep_theirs() {
        let plane = Source::of("Plane", &ground(80, 80, |x, y| 3.0 * x - y)).expect("a source");
        let far_from_the_edges = (24..56)
            .flat_map(|j| (24..56).map(move |i| (i, j)))
            .map(|(i, j)| f64::from(plane.relief.outer[j * 81 + i]).abs())
            .fold(0.0, f64::max);
        assert!(far_from_the_edges < 0.01, "{far_from_the_edges}");
        let bumps = Source::of(
            "Bumps",
            &ground(40, 40, |x, y| 4.0 * (x * 1.3).sin() * y.cos()),
        )
        .expect("a source");
        assert!(bumps.spread > 1.0, "{}", bumps.spread);
        assert_eq!(
            bumps.patches.len(),
            13 * 13,
            "every other cell of 25 each way"
        );
    }

    #[test]
    fn damaged_tiles_give_errors_or_their_own_chunks_never_a_panic() {
        let Some(data) = std::env::var_os("WOW_DATA") else {
            eprintln!("skipped: WOW_DATA is not set");
            return;
        };
        let chain = Chain::open(std::path::PathBuf::from(data)).expect("the install");
        let whole = chain
            .read("World\\Maps\\Azeroth\\Azeroth_32_48.adt")
            .expect("an Elwynn tile");
        let everything = |_: u32| true;
        assert_eq!(
            heights_of(&whole, (32, 48), everything).map(|c| c.len()),
            Ok(256)
        );
        let mut off_the_tile = whole.clone();
        let chunk = whole
            .windows(4)
            .position(|w| w == b"KNCM")
            .expect("a chunk");
        off_the_tile[chunk + 12..chunk + 16].copy_from_slice(&99u32.to_le_bytes());
        assert!(heights_of(&off_the_tile, (32, 48), everything).is_err());
        let mut read = 0;
        let mut copies: Vec<Vec<u8>> = (1..40)
            .map(|k| whole[..whole.len() * k / 40].to_vec())
            .collect();
        let mut at = 7usize;
        for _ in 0..400 {
            at = (at * 1_103_515_245 + 12_345) % whole.len();
            let mut b = whole.clone();
            b[at] ^= 1 << (at % 8);
            copies.push(b);
        }
        for b in &copies {
            if let Ok(chunks) = heights_of(b, (32, 48), everything) {
                read += 1;
                assert!(chunks.iter().all(|c| {
                    (512..528).contains(&c.chunk.0)
                        && (768..784).contains(&c.chunk.1)
                        && c.heights.len() == 145
                }));
            }
        }
        assert!(read > 0);
    }

    #[test]
    fn a_patch_lies_wholly_on_known_ground() {
        let mut g = ground(24, 20, |x, _| x);
        g.known[5 * 24 + 7] = false;
        let s = Source::of("Holed", &g).expect("a source");
        for &(i, j) in &s.patches {
            let (i, j) = (i as usize, j as usize);
            assert!(!(i..i + PATCH).contains(&7) || !(j..j + PATCH).contains(&5));
        }
        assert!(!s.patches.is_empty());
        g.known.fill(false);
        assert!(Source::of("None", &g).is_err());
    }
}
