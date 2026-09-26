//! Meshes World of Warcraft 1.12.1 ADT terrain and liquids, and answers point queries on them.

mod error;
mod liquid;
mod map;
mod mesh;
mod query;

pub use error::Error;
pub use liquid::{LiquidKind, LiquidMesh, build_liquid_mesh};
pub use map::{MapTiles, find_tile_near, load_tile_mesh, load_tiles_around};
pub use mesh::{
    ALPHA_MAP_SIZE, CHUNK_SIZE, ChunkMesh, DOODAD_SCALE_STEPS, Doodad, LAYER_REPEATS_PER_CHUNK,
    SHADOW_MAP_SIZE, TILE_SIZE, TileMesh, VERTICES, WmoInstance, adt_to_tile_mesh, cell_vertices,
    is_hole, placement_to_world, world_to_placement,
};
pub use query::{
    area_id_at, ground_effect_at, impassable_at, mcsh_shadowed_at, terrain_height_at, triangle_z_at,
};
