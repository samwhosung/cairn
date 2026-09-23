//! The map grid: 64×64 tiles of 533⅓ yards, each 16×16 chunks.

pub(crate) const TILES_PER_MAP: usize = 64;
const TILE_YARDS: f32 = 533.333_3;
const CHUNKS_PER_TILE: u32 = 16;
const CHUNK_YARDS: f32 = TILE_YARDS / CHUNKS_PER_TILE as f32;
const MAP_ORIGIN: f32 = 32.0 * TILE_YARDS;

/// World `(x, y)` to the ADT tile `(tile_x, tile_y)` holding it, clamped to `0..=63`. The axes
/// cross: `tile_x` runs along world y, `tile_y` along world x.
pub fn world_to_tile(world_x: f32, world_y: f32) -> (u32, u32) {
    let tile_x = ((MAP_ORIGIN - world_y) / TILE_YARDS) as u32;
    let tile_y = ((MAP_ORIGIN - world_x) / TILE_YARDS) as u32;
    (tile_x.min(63), tile_y.min(63))
}

/// World `(x, y)` to the chunk `(chunk_x, chunk_y)` on the map's 1024×1024 chunk grid, clamped
/// to `0..=1023`. The axes cross as in [`world_to_tile`], and `chunk >> 4` is that tile.
pub fn world_to_chunk(world_x: f32, world_y: f32) -> (u32, u32) {
    let max = TILES_PER_MAP as u32 * CHUNKS_PER_TILE - 1;
    let chunk_x = ((MAP_ORIGIN - world_y) / CHUNK_YARDS) as u32;
    let chunk_y = ((MAP_ORIGIN - world_x) / CHUNK_YARDS) as u32;
    (chunk_x.min(max), chunk_y.min(max))
}

/// The world `(x, y)` of a tile's max corner: the inverse of [`world_to_tile`].
pub fn tile_to_world(tile_x: u32, tile_y: u32) -> (f32, f32) {
    (
        MAP_ORIGIN - tile_y as f32 * TILE_YARDS,
        MAP_ORIGIN - tile_x as f32 * TILE_YARDS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f32 = 533.333_3;
    const OFFSET: f32 = 32.0 * T;

    #[test]
    fn the_chunk_index_nests_in_the_tile_index() {
        let (ox, oy) = tile_to_world(32, 44);
        for cx in 0..16u32 {
            for cy in 0..16u32 {
                let wy = oy - (cx as f32 + 0.5) * CHUNK_YARDS;
                let wx = ox - (cy as f32 + 0.5) * CHUNK_YARDS;
                let (chx, chy) = world_to_chunk(wx, wy);
                assert_eq!((chx >> 4, chy >> 4), world_to_tile(wx, wy));
                assert_eq!((chx, chy), (32 * 16 + cx, 44 * 16 + cy));
            }
        }
        assert_eq!(world_to_chunk(-1.0e6, -1.0e6), (1023, 1023));
        assert_eq!(world_to_chunk(1.0e6, 1.0e6), (0, 0));
    }

    #[test]
    fn world_origin_is_the_map_center() {
        assert_eq!(world_to_tile(0.0, 0.0), (32, 32));
    }

    #[test]
    fn tile_centers_round_trip() {
        for &(tx, ty) in &[(0, 0), (1, 10), (20, 31), (33, 33), (50, 62), (63, 63)] {
            let (cx, cy) = tile_to_world(tx, ty);
            let center = (cx - 0.5 * T, cy - 0.5 * T);
            assert_eq!(world_to_tile(center.0, center.1), (tx, ty), "({tx},{ty})");
        }
    }

    #[test]
    fn tile_to_world_crosses_the_axes() {
        assert_eq!(tile_to_world(0, 0), (OFFSET, OFFSET));
        let (wx, wy) = tile_to_world(1, 2);
        assert!((wx - (OFFSET - 2.0 * T)).abs() < 0.01);
        assert!((wy - (OFFSET - 1.0 * T)).abs() < 0.01);
    }

    #[test]
    fn world_to_tile_clamps_both_extremes() {
        assert_eq!(world_to_tile(1.0e9, 1.0e9), (0, 0));
        assert_eq!(world_to_tile(-1.0e9, -1.0e9), (63, 63));
    }
}
