use std::collections::HashMap;
use std::io::Cursor;

use m2::parse_m2;
use mpq::Chain;
use wmo::{ParsedWmo, parse_wmo};

use crate::{Error, model_path};

/// Collision triangles in the model's own coordinates, the space of
/// [`RenderSubmesh::positions`](crate::RenderSubmesh::positions). Empty when the source has no
/// collision geometry.
#[derive(Debug, Clone, Default)]
pub struct CollisionMesh {
    pub positions: Vec<[f32; 3]>,
    /// A triangle list into `positions`.
    pub indices: Vec<u32>,
}

impl CollisionMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Reads an M2's collision hull: the coarse solid the client collides with, apart from the
/// render mesh. Empty when the hull is absent or malformed.
pub fn load_m2_collision_hull(chain: &Chain, raw_path: &str) -> Result<CollisionMesh, Error> {
    let bytes = chain.read(&model_path(raw_path)).map_err(Error::Chain)?;
    parse_m2_collision_hull(&bytes)
}

/// [`load_m2_collision_hull`] from the file's bytes. Empty when the hull is absent or malformed:
/// a partial triangle or an index out of range.
pub fn parse_m2_collision_hull(bytes: &[u8]) -> Result<CollisionMesh, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    let rd = &format.model().raw_data;
    let positions: Vec<[f32; 3]> = rd
        .bounding_vertices
        .as_chunks::<12>()
        .0
        .iter()
        .map(|c| {
            [
                f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                f32::from_le_bytes([c[8], c[9], c[10], c[11]]),
            ]
        })
        .collect();
    let n = positions.len() as u32;
    let indices: Vec<u32> = rd
        .bounding_triangles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
        .collect();
    if indices.is_empty() || !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i >= n) {
        return Ok(CollisionMesh::default());
    }
    Ok(CollisionMesh { positions, indices })
}

/// `MOPY` flag of a detail face: walking collision passes through it, the camera does not.
const MOPY_DETAIL: u8 = 0x04;
/// `MOPY` flag of a face the camera passes through; walking collides with it.
const MOPY_NOCAMCOLLIDE: u8 = 0x02;

/// Reads a WMO's walking-collision triangles from every group file into one mesh, in the WMO's
/// own coordinates. A group file that is missing or does not parse is skipped.
pub fn load_wmo_collision_tris(chain: &Chain, raw_path: &str) -> Result<CollisionMesh, Error> {
    let root_path = raw_path.to_ascii_lowercase();
    let bytes = chain.read(&root_path).map_err(Error::Chain)?;
    let ParsedWmo::Root(root) = parse_wmo(&bytes).map_err(Error::Wmo)? else {
        return Err(Error::NotWmoRoot);
    };
    let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
    let (mut positions, mut indices): (Vec<[f32; 3]>, Vec<u32>) = (Vec::new(), Vec::new());
    for gi in 0..root.n_groups {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read(&group_path) else {
            continue;
        };
        accumulate_wmo_group_collision(&gbytes, &mut positions, &mut indices);
    }
    Ok(CollisionMesh { positions, indices })
}

/// Appends one WMO group file's walking-collision triangles: every face but detail faces.
pub fn accumulate_wmo_group_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(group_bytes, MOPY_DETAIL, 0, positions, indices);
}

/// Appends one WMO group file's camera-collision triangles: every face the camera does not pass
/// through, detail faces included.
pub fn accumulate_wmo_group_camera_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(group_bytes, MOPY_NOCAMCOLLIDE, 0, positions, indices);
}

/// Appends the faces only the camera collides with: detail faces the camera does not pass
/// through.
pub fn accumulate_wmo_group_camera_only_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(
        group_bytes,
        MOPY_NOCAMCOLLIDE,
        MOPY_DETAIL,
        positions,
        indices,
    );
}

fn accumulate_wmo_group_faces(
    group_bytes: &[u8],
    skip_mask: u8,
    require_mask: u8,
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(group_bytes) else {
        return;
    };
    let n_tri = group.vertex_indices.len() / 3;
    let mut map: HashMap<u16, u32> = HashMap::new();
    for t in 0..n_tri {
        let Some(mopy) = group.material_info.get(t) else {
            continue;
        };
        if mopy.flags & skip_mask != 0 || mopy.flags & require_mask != require_mask {
            continue;
        }
        for k in 0..3 {
            let Some(&vidx) = group.vertex_indices.get(t * 3 + k) else {
                continue;
            };
            let Some(p) = group.vertex_positions.get(vidx as usize) else {
                continue;
            };
            let global = *map.entry(vidx).or_insert_with(|| {
                positions.push([p.x, p.y, p.z]);
                (positions.len() - 1) as u32
            });
            indices.push(global);
        }
    }
}
