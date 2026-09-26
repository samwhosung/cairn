//! Every map's tiles read in parallel, each boiled down to what it paints and places before the
//! next is kept, so the whole install fits in memory.

use std::io::Cursor;

use atlas::layer_weights;
use dbc::{DbcParser, FieldType, Schema, SchemaField, Value};
use mpq::Chain;
use rayon::prelude::*;
use terrain::{ALPHA_MAP_SIZE, CHUNK_SIZE, ChunkMesh, Doodad, LiquidKind, WmoInstance};
use wdt::{GlobalWmo, WdtReader};

const MAP_DBC: &str = "DBFilesClient\\Map.dbc";
const MAP_FIELDS: usize = 42;
const TEXELS: usize = (ALPHA_MAP_SIZE * ALPHA_MAP_SIZE) as usize;
const OUTER_ROW: usize = 17;

/// A map `Map.dbc` lists and the install has a table of tiles for.
pub(crate) struct MapTiles {
    pub(crate) id: u32,
    pub(crate) directory: String,
    pub(crate) tiles: Vec<(u32, u32)>,
    pub(crate) global_wmo: Option<GlobalWmo>,
}

/// One terrain chunk: its area, its middle, what paints it and what water lies on it.
pub(crate) struct Chunk {
    pub(crate) area: u32,
    pub(crate) middle: [f32; 3],
    /// Each layer's texture and the texels of the chunk's 64×64 it shows on.
    pub(crate) paint: Vec<(String, f32)>,
    /// Wet cells of the chunk's 8×8, by [`WATERS`].
    pub(crate) water: [u16; 4],
}

/// The kinds of water a cell can hold, as the catalog names them.
pub(crate) const WATERS: [&str; 4] = ["water", "ocean", "magma", "slime"];

/// A placement a tile lists, with the area under it when it stands on this tile.
pub(crate) struct Listed<T> {
    pub(crate) placed: T,
    pub(crate) area: Option<u32>,
}

/// One tile, read and boiled down.
pub(crate) struct Tile {
    pub(crate) map: usize,
    pub(crate) at: (u32, u32),
    pub(crate) chunks: Vec<Chunk>,
    pub(crate) doodads: Vec<Listed<Doodad>>,
    pub(crate) wmos: Vec<Listed<WmoInstance>>,
}

pub(crate) fn maps(chain: &Chain) -> Result<Vec<MapTiles>, String> {
    let bytes = chain.read(MAP_DBC).map_err(|e| e.to_string())?;
    let mut schema = Schema::new("Map");
    schema.add_field(SchemaField::new("id", FieldType::UInt32));
    schema.add_field(SchemaField::new("directory", FieldType::String));
    schema.add_field(SchemaField::new_array(
        "unread",
        FieldType::UInt32,
        MAP_FIELDS - 2,
    ));
    let rows = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|e| format!("{MAP_DBC}: {e}"))?;
    let mut maps: Vec<MapTiles> = rows
        .records()
        .iter()
        .filter_map(|r| {
            let (Some(Value::UInt32(id)), Some(Value::StringRef(dir))) =
                (r.get_value(0), r.get_value(1))
            else {
                return None;
            };
            let directory = rows.get_string(*dir).ok()?.into_owned();
            let wdt = chain
                .read(&format!("World\\Maps\\{directory}\\{directory}.wdt"))
                .ok()?;
            let wdt = WdtReader::new(Cursor::new(wdt)).read().ok()?;
            let mut tiles = Vec::new();
            for y in 0..64 {
                for x in 0..64 {
                    if wdt.get_tile(x, y).is_some_and(|t| t.has_adt) {
                        tiles.push((x as u32, y as u32));
                    }
                }
            }
            Some(MapTiles {
                id: *id,
                directory,
                tiles,
                global_wmo: wdt.global_wmo().cloned(),
            })
        })
        .collect();
    maps.sort_by_key(|m| m.id);
    Ok(maps)
}

/// Every tile of every map, in map order and then rows from the north-west. A tile that fails to
/// read or mesh is left out, as the client would draw nothing there.
pub(crate) fn tiles(chain: &Chain, maps: &[MapTiles]) -> Vec<Tile> {
    let wanted: Vec<(usize, (u32, u32))> = maps
        .iter()
        .enumerate()
        .flat_map(|(i, m)| m.tiles.iter().map(move |&t| (i, t)))
        .collect();
    wanted
        .par_iter()
        .filter_map(|&(map, (x, y))| {
            let dir = &maps[map].directory;
            let bytes = chain
                .read(&format!("World\\Maps\\{dir}\\{dir}_{x}_{y}.adt"))
                .ok()?;
            let mesh = terrain::adt_to_tile_mesh(&bytes).ok()?;
            let area = |p: [f32; 3]| terrain::area_id_at(&mesh.chunks, p);
            let doodads = mesh
                .doodads
                .iter()
                .map(|d| Listed {
                    area: area(d.position),
                    placed: d.clone(),
                })
                .collect();
            let wmos = mesh
                .wmos
                .iter()
                .map(|w| Listed {
                    area: area(w.position),
                    placed: w.clone(),
                })
                .collect();
            Some(Tile {
                map,
                at: (x, y),
                chunks: mesh.chunks.iter().map(chunk).collect(),
                doodads,
                wmos,
            })
        })
        .collect()
}

fn chunk(c: &ChunkMesh) -> Chunk {
    let n = c.layer_textures.len().min(4);
    let mut shown = [0f32; 4];
    for texel in 0..TEXELS {
        let w = layer_weights(c.alpha_map.as_deref(), texel, n);
        for (sum, w) in shown.iter_mut().zip(w) {
            *sum += w;
        }
    }
    let paint = c
        .layer_textures
        .iter()
        .take(n)
        .zip(shown)
        .map(|(t, w)| (t.clone(), w))
        .collect();
    let mut water = [0u16; 4];
    for l in &c.liquids {
        let slot = match l.kind {
            LiquidKind::Still | LiquidKind::Rapids => 0,
            LiquidKind::Ocean => 1,
            LiquidKind::Magma => 2,
            LiquidKind::Slime => 3,
        };
        water[slot] += l.wet.iter().filter(|w| **w).count() as u16;
    }
    let [x, y, _] = c.positions[0];
    let outer = (0..9).flat_map(|r| (0..9).map(move |k| r * OUTER_ROW + k));
    let z = outer.map(|i| c.positions[i][2]).sum::<f32>() / 81.0;
    Chunk {
        area: c.area_id,
        middle: [x - CHUNK_SIZE / 2.0, y - CHUNK_SIZE / 2.0, z],
        paint,
        water,
    }
}
