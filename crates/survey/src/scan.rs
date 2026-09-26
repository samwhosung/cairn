use std::io::Cursor;

use atlas::layer_weights;
use dbc::{DbcParser, FieldType, Schema, SchemaField, Value};
use mpq::Chain;
use rayon::prelude::*;
use terrain::{ALPHA_MAP_SIZE, CHUNK_SIZE, ChunkMesh, Doodad, LiquidKind, VERTICES, WmoInstance};
use wdt::{GlobalWmo, WdtReader};

use crate::WetCells;

const MAP_DBC: &str = "DBFilesClient\\Map.dbc";
const MAP_FIELDS: usize = 42;
const CHUNKS_A_SIDE: u32 = 16;
const ROW_STRIDE: usize = 17;
const MOST_OF_A_CHUNK: f32 = 0.9999;
pub(crate) const TEXELS_PER_CHUNK: usize = (ALPHA_MAP_SIZE * ALPHA_MAP_SIZE) as usize;

pub(crate) struct MapTiles {
    pub(crate) id: u32,
    pub(crate) directory: String,
    pub(crate) tiles: Vec<(u32, u32)>,
    pub(crate) global_wmo: Option<GlobalWmo>,
}

pub(crate) struct ChunkSummary {
    pub(crate) area: u32,
    pub(crate) paint: Vec<Paint>,
    pub(crate) wet_cells: WetCells,
}

pub(crate) struct Paint {
    pub(crate) texture: String,
    pub(crate) texels: f32,
}

pub(crate) struct Listed<T> {
    pub(crate) placed: T,
    pub(crate) here: Option<Underfoot>,
}

/// The ground under a point: its chunk's area, the texture that shows most at the point, and the
/// slope of the cell holding it, in degrees.
#[derive(Clone, Debug, PartialEq)]
pub struct Underfoot {
    pub area: u32,
    pub texture: Option<String>,
    pub slope: Option<f32>,
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
            let here = |p: [f32; 3]| underfoot(&mesh.chunks, (x, y), p);
            let doodads = mesh
                .doodads
                .iter()
                .map(|d| Listed {
                    here: here(d.position),
                    placed: d.clone(),
                })
                .collect();
            let wmos = mesh
                .wmos
                .iter()
                .map(|w| Listed {
                    here: here(w.position),
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

/// The ground under `p` on the tile `Map_x_y.adt` numbers as `tile`, whose chunks these are; `None`
/// off them. A point on the edge between two chunks lies in the one south or east of it, as
/// `world_to_chunk` places it.
pub fn underfoot(
    chunks: &[ChunkMesh],
    (tile_x, tile_y): (u32, u32),
    p: [f32; 3],
) -> Option<Underfoot> {
    let (chunk_x, chunk_y) = wdt::world_to_chunk(p[0], p[1]);
    let column = chunk_x.checked_sub(tile_x * CHUNKS_A_SIDE)?;
    let row = chunk_y.checked_sub(tile_y * CHUNKS_A_SIDE)?;
    let c = chunks
        .iter()
        .find(|c| c.index_x == column && c.index_y == row)?;
    let nw = c.positions.first().copied().unwrap_or(p);
    let across = |along: f32| (along / CHUNK_SIZE).clamp(0.0, MOST_OF_A_CHUNK);
    let (south, east) = (across(nw[0] - p[0]), across(nw[1] - p[1]));
    let layers = c.layer_textures.len().min(4);
    let texture = (layers > 0).then(|| {
        let side = ALPHA_MAP_SIZE as f32;
        let texel = (south * side) as usize * ALPHA_MAP_SIZE as usize + (east * side) as usize;
        let w = layer_weights(c.alpha_map.as_deref(), texel, layers);
        let top = (0..layers)
            .max_by(|&a, &b| w[a].total_cmp(&w[b]))
            .unwrap_or(0);
        c.layer_textures[top].clone()
    });
    Some(Underfoot {
        area: c.area_id,
        texture,
        slope: cell_slope_degrees(c, (south * 8.0) as usize, (east * 8.0) as usize),
    })
}

fn cell_slope_degrees(c: &ChunkMesh, row: usize, column: usize) -> Option<f32> {
    if c.positions.len() != VERTICES {
        return None;
    }
    let h = |r: usize, k: usize| c.positions[r * ROW_STRIDE + k][2];
    let cell = CHUNK_SIZE / 8.0;
    let (r, k) = (row, column);
    let south = (h(r + 1, k) + h(r + 1, k + 1) - h(r, k) - h(r, k + 1)) / (2.0 * cell);
    let east = (h(r, k + 1) + h(r + 1, k + 1) - h(r, k) - h(r + 1, k)) / (2.0 * cell);
    Some(south.hypot(east).atan().to_degrees())
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
    ChunkSummary {
        area: c.area_id,
        paint,
        wet_cells,
    }
}

#[cfg(test)]
mod tests {
    use terrain::TILE_SIZE;

    use super::*;

    fn chunk(row: u32, column: u32, area: u32) -> ChunkMesh {
        ChunkMesh {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            holes: 0,
            base_texture: None,
            layer_textures: Vec::new(),
            layer_effect_ids: Vec::new(),
            alpha_map: None,
            shadow: None,
            pred_tex: [0; 64],
            no_effect_doodad: [false; 64],
            index_x: column,
            index_y: row,
            area_id: area,
            impassable: false,
            liquids: Vec::new(),
        }
    }

    fn area_under(chunks: &[ChunkMesh], tile: (u32, u32), p: [f32; 3]) -> Option<u32> {
        underfoot(chunks, tile, p).map(|u| u.area)
    }

    #[test]
    fn a_point_on_an_edge_lies_in_the_chunk_south_of_it() {
        let chunks = [chunk(0, 0, 1), chunk(1, 0, 2)];
        let tile = (32, 32);
        let north_edge = 32.0 * TILE_SIZE - 32.0 * TILE_SIZE;
        let edge = north_edge - TILE_SIZE / 16.0;
        let y = -1.0;
        assert_eq!(area_under(&chunks, tile, [edge + 1.0, y, 0.0]), Some(1));
        assert_eq!(area_under(&chunks, tile, [edge, y, 0.0]), Some(2));
        assert_eq!(area_under(&chunks, tile, [edge - 1.0, y, 0.0]), Some(2));
        assert_eq!(
            area_under(&chunks, (31, 32), [edge, y, 0.0]),
            None,
            "another tile's"
        );
    }

    #[test]
    fn underfoot_is_the_texture_showing_most_and_the_cell_slope() {
        let mut c = chunk(0, 0, 7);
        let cell = CHUNK_SIZE / 8.0;
        let rise = 30f32.to_radians().tan();
        c.positions = (0..VERTICES)
            .map(|i| {
                let (row, at) = (i / ROW_STRIDE, i % ROW_STRIDE);
                let (south, east) = if at < 9 {
                    (row as f32, at as f32)
                } else {
                    (row as f32 + 0.5, (at - 9) as f32 + 0.5)
                };
                [-south * cell, -east * cell, south * cell * rise]
            })
            .collect();
        c.layer_textures = vec!["Base.blp".into(), "East.blp".into()];
        let side = ALPHA_MAP_SIZE as usize;
        c.alpha_map = Some(
            (0..side * side)
                .flat_map(|t| [if t % side >= side / 2 { 255 } else { 0 }, 0, 0, 0])
                .collect(),
        );
        let at = |south: f32, east: f32| {
            underfoot(
                std::slice::from_ref(&c),
                (32, 32),
                [-south * CHUNK_SIZE, -east * CHUNK_SIZE, 0.0],
            )
            .expect("on the chunk")
        };
        let west = at(0.3, 0.2);
        assert_eq!(west.area, 7);
        assert_eq!(west.texture.as_deref(), Some("Base.blp"));
        assert!((west.slope.expect("a full grid") - 30.0).abs() < 0.01);
        assert_eq!(at(0.3, 0.8).texture.as_deref(), Some("East.blp"));
    }
}
