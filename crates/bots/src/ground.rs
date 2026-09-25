use std::collections::HashMap;
use std::io::Cursor;

use terrain::{ChunkMesh, LiquidMesh, area_id_at, load_tile_mesh, terrain_height_at};

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";
const AREA_ID: usize = 0;
const AREA_PARENT: usize = 2;

pub struct Ground {
    tiles: HashMap<(u32, u32), Vec<ChunkMesh>>,
    parent: HashMap<u32, u32>,
    flat: Option<f32>,
}

impl Ground {
    /// Loads `map`'s tiles `x.0..=x.1` by `y.0..=y.1`, numbered as in `Map_<x>_<y>.adt`; a tile
    /// that is missing or fails to build is left out.
    pub fn load(
        chain: &mpq::Chain,
        map: &str,
        x: (u32, u32),
        y: (u32, u32),
    ) -> Result<Self, String> {
        let wanted: Vec<(u32, u32)> = (x.0..=x.1)
            .flat_map(|tx| (y.0..=y.1).map(move |ty| (tx, ty)))
            .collect();
        let tiles: HashMap<(u32, u32), Vec<ChunkMesh>> = std::thread::scope(|s| {
            let jobs: Vec<_> = wanted
                .iter()
                .map(|&(tx, ty)| s.spawn(move || (tx, ty, load_tile_mesh(chain, map, tx, ty))))
                .collect();
            jobs.into_iter()
                .filter_map(|j| j.join().ok())
                .filter_map(|(tx, ty, mesh)| Some(((tx, ty), strip_for_queries(mesh.ok()?.chunks))))
                .collect()
        });
        if tiles.is_empty() {
            return Err(format!("no terrain on {map} tiles {x:?} by {y:?}"));
        }
        Ok(Self {
            tiles,
            parent: area_parents(chain)?,
            flat: None,
        })
    }

    /// Level ground at height `z` everywhere, with no water and no zones.
    pub fn flat(z: f32) -> Self {
        Self {
            tiles: HashMap::new(),
            parent: HashMap::new(),
            flat: Some(z),
        }
    }

    fn chunks(&self, x: f32, y: f32) -> Option<&[ChunkMesh]> {
        self.tiles.get(&wdt::world_to_tile(x, y)).map(Vec::as_slice)
    }

    /// The terrain's height under world `(x, y)`; `None` off the loaded tiles or over a hole.
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        if let Some(z) = self.flat {
            return Some(z);
        }
        terrain_height_at(self.chunks(x, y)?, [x, y, 0.0])
    }

    /// The top of the ground or of the water over it, whichever is higher.
    pub fn surface(&self, x: f32, y: f32) -> Option<f32> {
        let ground = self.height(x, y)?;
        Some(self.water(x, y).map_or(ground, |w| w.max(ground)))
    }

    /// The height of the terrain's water over world `(x, y)`, where there is any.
    pub fn water(&self, x: f32, y: f32) -> Option<f32> {
        self.chunks(x, y)?
            .iter()
            .flat_map(|c| &c.liquids)
            .find_map(|l| water_at(l, x, y))
    }

    /// The top-level zone holding world `(x, y)`, as an `AreaTable` id.
    pub fn zone(&self, x: f32, y: f32) -> Option<u32> {
        let mut area = area_id_at(self.chunks(x, y)?, [x, y, 0.0])?;
        for _ in 0..8 {
            match self.parent.get(&area) {
                Some(&p) if p != 0 => area = p,
                _ => break,
            }
        }
        Some(area)
    }
}

/// A liquid's height over `(x, y)`, interpolated from its cell's four corners as the client
/// samples it; `None` off the liquid or over a dry cell.
fn water_at(l: &LiquidMesh, x: f32, y: f32) -> Option<f32> {
    let [cols, rows] = l.grid.map(|n| n as usize);
    let nw = *l.positions.first()?;
    let cell = nw[1] - l.positions.get(1)?[1];
    let (down, across) = ((nw[0] - x) / cell, (nw[1] - y) / cell);
    let inside = |f: f32, n: usize| (0.0..(n - 1) as f32).contains(&f);
    if cell <= 0.0 || !inside(down, rows) || !inside(across, cols) {
        return None;
    }
    let (r, c) = (down as usize, across as usize);
    if !l.wet.get(r * (cols - 1) + c).copied().unwrap_or(false) {
        return None;
    }
    let h = |r: usize, c: usize| l.positions[r * cols + c][2];
    let (dr, dc) = (down - r as f32, across - c as f32);
    let north = h(r, c) + (h(r, c + 1) - h(r, c)) * dc;
    let south = h(r + 1, c) + (h(r + 1, c + 1) - h(r + 1, c)) * dc;
    Some(north + (south - north) * dr)
}

fn strip_for_queries(mut chunks: Vec<ChunkMesh>) -> Vec<ChunkMesh> {
    for c in &mut chunks {
        c.normals = Vec::new();
        c.uvs = Vec::new();
        c.layer_textures = Vec::new();
        c.layer_effect_ids = Vec::new();
        c.base_texture = None;
        c.alpha_map = None;
        c.shadow = None;
        for l in &mut c.liquids {
            l.uvs = Vec::new();
            l.depths = Vec::new();
            l.indices = Vec::new();
        }
    }
    chunks
}

fn area_parents(chain: &mpq::Chain) -> Result<HashMap<u32, u32>, String> {
    let bytes = chain
        .read(AREA_TABLE)
        .map_err(|e| format!("{AREA_TABLE}: {e}"))?;
    let parser = dbc::DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .map_err(|e| format!("{AREA_TABLE}: {e}"))?;
    let mut schema = dbc::Schema::new("AreaTable");
    for _ in 0..parser.header().field_count {
        schema.add_field(dbc::SchemaField::new("", dbc::FieldType::UInt32));
    }
    let records = parser
        .with_schema(schema)
        .and_then(|p| p.parse_records())
        .map_err(|e| format!("{AREA_TABLE}: {e}"))?;
    let u32_at = |r: &dbc::Record, i: usize| match r.get_value(i) {
        Some(dbc::Value::UInt32(v)) => Some(*v),
        _ => None,
    };
    Ok(records
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, AREA_ID)?, u32_at(r, AREA_PARENT)?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use terrain::LiquidKind;

    use super::*;

    fn pond() -> LiquidMesh {
        let cell = 4.0;
        let positions = (0..9 * 9)
            .map(|n| {
                let (row, col) = ((n / 9) as f32, (n % 9) as f32);
                [100.0 - row * cell, 50.0 - col * cell, row + col]
            })
            .collect();
        let mut wet = vec![false; 64];
        wet[9] = true;
        LiquidMesh {
            grid: [9, 9],
            wet,
            shared: vec![false; 64],
            positions,
            uvs: Vec::new(),
            depths: Vec::new(),
            indices: Vec::new(),
            sound_nibble: 0,
            material_id: None,
            kind: LiquidKind::Still,
        }
    }

    #[test]
    fn water_is_read_bilinearly_from_its_cells_corners_and_only_where_wet() {
        let pond = pond();
        let at = water_at(&pond, 100.0 - 4.0 - 1.0, 50.0 - 4.0 - 3.0);
        assert_eq!(at, Some(1.25 + 1.75));
        assert_eq!(water_at(&pond, 99.0, 49.0), None, "a dry cell");
        assert_eq!(water_at(&pond, 101.0, 49.0), None, "off the liquid");
    }
}
