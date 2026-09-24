use std::collections::HashMap;
use std::io::Cursor;

use terrain::{ChunkMesh, area_id_at, load_tile_mesh, terrain_height_at};

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";
const AREA_ID: usize = 0;
const AREA_PARENT: usize = 2;

/// Terrain heights and zones over a block of tiles, read once from the install.
pub struct Ground {
    tiles: HashMap<(u32, u32), Vec<ChunkMesh>>,
    parent: HashMap<u32, u32>,
}

impl Ground {
    /// Loads `map`'s tiles `x0..=x1` by `y0..=y1`, as in `Map_<x>_<y>.adt`; a tile that is
    /// missing or fails to build is left out.
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
                .filter_map(|(tx, ty, mesh)| Some(((tx, ty), heights_only(mesh.ok()?.chunks))))
                .collect()
        });
        if tiles.is_empty() {
            return Err(format!("no terrain on {map} tiles {x:?} by {y:?}"));
        }
        Ok(Self {
            tiles,
            parent: area_parents(chain)?,
        })
    }

    /// Ground with no tiles: every height query misses, so a mover keeps its height.
    #[cfg(test)]
    pub fn none() -> Self {
        Self {
            tiles: HashMap::new(),
            parent: HashMap::new(),
        }
    }

    fn chunks(&self, x: f32, y: f32) -> Option<&[ChunkMesh]> {
        self.tiles.get(&wdt::world_to_tile(x, y)).map(Vec::as_slice)
    }

    /// The terrain's height under world `(x, y)`; `None` off the loaded tiles or over a hole.
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        terrain_height_at(self.chunks(x, y)?, [x, y, 0.0])
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

/// Keeps what a height or zone query reads.
fn heights_only(mut chunks: Vec<ChunkMesh>) -> Vec<ChunkMesh> {
    for c in &mut chunks {
        c.normals = Vec::new();
        c.uvs = Vec::new();
        c.layer_textures = Vec::new();
        c.layer_effect_ids = Vec::new();
        c.base_texture = None;
        c.alpha_map = None;
        c.shadow = None;
        c.liquids = Vec::new();
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
