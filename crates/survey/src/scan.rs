use std::io::Cursor;

use atlas::layer_weights;
use dbc::{DbcParser, FieldType, Schema, SchemaField, Value};
use mpq::Chain;
use rayon::prelude::*;
use terrain::{ALPHA_MAP_SIZE, CHUNK_SIZE, ChunkMesh, Doodad, LiquidKind, WmoInstance};
use wdt::{GlobalWmo, WdtReader};

use crate::WetCells;

const MAP_DBC: &str = "DBFilesClient\\Map.dbc";
const MAP_FIELDS: usize = 42;
pub(crate) const TEXELS_PER_CHUNK: usize = (ALPHA_MAP_SIZE * ALPHA_MAP_SIZE) as usize;
const OUTER_ROW: usize = 17;
const OUTER_VERTICES: usize = 81;

pub(crate) struct MapTiles {
    pub(crate) id: u32,
    pub(crate) directory: String,
    pub(crate) tiles: Vec<(u32, u32)>,
    pub(crate) global_wmo: Option<GlobalWmo>,
}

pub(crate) struct ChunkSummary {
    pub(crate) area: u32,
    pub(crate) middle: [f32; 3],
    pub(crate) paint: Vec<Paint>,
    pub(crate) wet_cells: WetCells,
}

pub(crate) struct Paint {
    pub(crate) texture: String,
    pub(crate) texels: f32,
}

pub(crate) struct Listed<T> {
    pub(crate) placed: T,
    pub(crate) area_here: Option<u32>,
}

pub(crate) struct TileSummary {
    pub(crate) map: usize,
    pub(crate) at: (u32, u32),
    pub(crate) chunks: Vec<ChunkSummary>,
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

pub(crate) fn tiles(chain: &Chain, maps: &[MapTiles]) -> Vec<TileSummary> {
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
            let area_here = |p: [f32; 3]| terrain::area_id_at(&mesh.chunks, p);
            let doodads = mesh
                .doodads
                .iter()
                .map(|d| Listed {
                    area_here: area_here(d.position),
                    placed: d.clone(),
                })
                .collect();
            let wmos = mesh
                .wmos
                .iter()
                .map(|w| Listed {
                    area_here: area_here(w.position),
                    placed: w.clone(),
                })
                .collect();
            Some(TileSummary {
                map,
                at: (x, y),
                chunks: mesh.chunks.iter().map(chunk).collect(),
                doodads,
                wmos,
            })
        })
        .collect()
}

fn chunk(c: &ChunkMesh) -> ChunkSummary {
    let layers = c.layer_textures.len().min(4);
    let mut shown = [0f32; 4];
    for texel in 0..TEXELS_PER_CHUNK {
        let w = layer_weights(c.alpha_map.as_deref(), texel, layers);
        for (sum, w) in shown.iter_mut().zip(w) {
            *sum += w;
        }
    }
    let paint = c
        .layer_textures
        .iter()
        .take(layers)
        .zip(shown)
        .map(|(texture, texels)| Paint {
            texture: texture.clone(),
            texels,
        })
        .collect();
    let mut wet_cells = WetCells::default();
    for l in &c.liquids {
        let wet = l.wet.iter().filter(|w| **w).count() as u32;
        match l.kind {
            LiquidKind::Still | LiquidKind::Rapids => wet_cells.water += wet,
            LiquidKind::Ocean => wet_cells.ocean += wet,
            LiquidKind::Magma => wet_cells.magma += wet,
            LiquidKind::Slime => wet_cells.slime += wet,
        }
    }
    let [x, y, _] = c.positions[0];
    let outer = (0..9).flat_map(|r| (0..9).map(move |k| r * OUTER_ROW + k));
    let z = outer.map(|i| c.positions[i][2]).sum::<f32>() / OUTER_VERTICES as f32;
    ChunkSummary {
        area: c.area_id,
        middle: [x - CHUNK_SIZE / 2.0, y - CHUNK_SIZE / 2.0, z],
        paint,
        wet_cells,
    }
}
