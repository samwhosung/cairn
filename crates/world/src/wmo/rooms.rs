use std::collections::HashMap;

use model::{
    NO_GROUP_LIQUID, WmoPortalInfo, WmoPortalRef, WmoRoot,
    accumulate_wmo_group_camera_only_collision, accumulate_wmo_group_collision, wmo_group_header,
    wmo_group_liquid_mesh,
};
use terrain::{LiquidKind, LiquidMesh};

pub(crate) type Triangle = [[f32; 3]; 3];
pub type Bounds = ([f32; 3], [f32; 3]);

/// A group's flags, its box from the root, and its slice of the portal refs.
#[derive(Clone, Copy, Debug)]
pub struct WmoGroupNav {
    pub flags: u32,
    pub wmo_group_id: u32,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub ref_start: u16,
    pub ref_count: u16,
    /// The root's entry for the group sets neither exterior bit: its liquid fogs as indoors.
    pub interior: bool,
    /// The liquid the whole group stands under, surface or not.
    pub flooded: Option<LiquidKind>,
    /// The fogs of [`super::WmoModel::fogs`] that may fog the room.
    pub fog_indices: [u8; 4],
}

/// A building's rooms, in its own space: each group's box and flags, the portals between them, the
/// faces that say which group a point stands in, and each group's liquid.
pub struct WmoRooms {
    /// The root's key into `WMOAreaTable`; `0` for none.
    pub wmo_id: u32,
    /// Per group, by its index in the root.
    pub group_nav: Vec<WmoGroupNav>,
    pub portal_vertices: Vec<[f32; 3]>,
    pub portal_infos: Vec<WmoPortalInfo>,
    pub portal_refs: Vec<WmoPortalRef>,
    /// Per group: the faces a walker collides with.
    pub group_collision_tris: Vec<Vec<Triangle>>,
    /// Per group: the faces only the camera collides with.
    pub group_camera_only_tris: Vec<Vec<Triangle>>,
    /// Per group: the box of its collision faces, `None` without any.
    pub group_collision_bounds: Vec<Option<Bounds>>,
    /// Per group: its liquid surface. A cell two groups both hold is kept by the lower index only.
    pub group_liquids: Vec<Option<LiquidMesh>>,
}

impl WmoRooms {
    pub fn has_portals(&self) -> bool {
        !self.portal_refs.is_empty() && !self.portal_infos.is_empty()
    }

    pub(crate) fn owns_its_pools(&self) -> bool {
        self.has_portals() || self.wmo_id != 0
    }
}

impl WmoGroupNav {
    pub fn has_box(&self) -> bool {
        self.bbox_min[0] <= self.bbox_max[0]
    }
}

pub(crate) struct RoomsBuilder(WmoRooms);

impl RoomsBuilder {
    pub(crate) fn new(root: &WmoRoot, root_bytes: &[u8]) -> Self {
        let groups = root.group_count() as usize;
        let group_nav = (0..groups)
            .map(|gi| {
                let info = root.group_infos().get(gi);
                let (bbox_min, bbox_max) = info
                    .map_or(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |g| {
                        (g.bbox_min, g.bbox_max)
                    });
                WmoGroupNav {
                    flags: 0,
                    wmo_group_id: 0,
                    bbox_min,
                    bbox_max,
                    ref_start: 0,
                    ref_count: 0,
                    interior: info.is_some_and(|g| g.interior),
                    flooded: None,
                    fog_indices: [0; 4],
                }
            })
            .collect();
        let portals = root.portals();
        Self(WmoRooms {
            wmo_id: model::wmo_root_id(root_bytes),
            group_nav,
            portal_vertices: portals.vertices.clone(),
            portal_infos: portals.infos.clone(),
            portal_refs: portals.refs.clone(),
            group_collision_tris: vec![Vec::new(); groups],
            group_camera_only_tris: vec![Vec::new(); groups],
            group_collision_bounds: vec![None; groups],
            group_liquids: vec![None; groups],
        })
    }

    pub(crate) fn add_group(&mut self, gi: usize, group_bytes: &[u8]) {
        let rooms = &mut self.0;
        if gi >= rooms.group_nav.len() {
            return;
        }
        if let Some(h) = wmo_group_header(group_bytes) {
            let nav = &mut rooms.group_nav[gi];
            nav.flags = h.flags;
            nav.wmo_group_id = h.area_table_id;
            nav.ref_start = h.portal_ref_start;
            nav.ref_count = h.portal_ref_count;
            nav.fog_indices = h.fog_indices;
            nav.flooded = (h.group_liquid != NO_GROUP_LIQUID)
                .then(|| model::LiquidKind::from_nibble((h.group_liquid & 0xf) as u8))
                .flatten()
                .map(liquid_kind);
        }
        let (mut pos, mut idx) = (Vec::new(), Vec::new());
        accumulate_wmo_group_collision(group_bytes, &mut pos, &mut idx);
        rooms.group_collision_tris[gi] = triangles(&pos, &idx);
        rooms.group_collision_bounds[gi] = bounds(rooms.group_collision_tris[gi].iter().flatten());
        let (mut pos, mut idx) = (Vec::new(), Vec::new());
        accumulate_wmo_group_camera_only_collision(group_bytes, &mut pos, &mut idx);
        rooms.group_camera_only_tris[gi] = triangles(&pos, &idx);
        rooms.group_liquids[gi] = wmo_group_liquid_mesh(group_bytes).map(liquid_mesh);
    }

    pub(crate) fn finish(mut self) -> WmoRooms {
        resolve_shared_liquid_cells(&mut self.0.group_liquids);
        self.0
    }
}

pub(crate) fn bounds<'a>(points: impl Iterator<Item = &'a [f32; 3]>) -> Option<Bounds> {
    points.fold(None, |acc, v| {
        let (mut min, mut max) = acc.unwrap_or((*v, *v));
        for a in 0..3 {
            min[a] = min[a].min(v[a]);
            max[a] = max[a].max(v[a]);
        }
        Some((min, max))
    })
}

fn triangles(positions: &[[f32; 3]], indices: &[u32]) -> Vec<Triangle> {
    indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|t| {
            Some([
                *positions.get(t[0] as usize)?,
                *positions.get(t[1] as usize)?,
                *positions.get(t[2] as usize)?,
            ])
        })
        .collect()
}

fn liquid_kind(kind: model::LiquidKind) -> LiquidKind {
    match kind {
        model::LiquidKind::Still => LiquidKind::Still,
        model::LiquidKind::Rapids => LiquidKind::Rapids,
        model::LiquidKind::Ocean => LiquidKind::Ocean,
        model::LiquidKind::Magma => LiquidKind::Magma,
        model::LiquidKind::Slime => LiquidKind::Slime,
    }
}

fn liquid_mesh(m: model::LiquidMesh) -> LiquidMesh {
    LiquidMesh {
        grid: m.grid,
        wet: m.wet,
        shared: m.shared,
        positions: m.positions,
        uvs: m.uvs,
        depths: m.depths,
        indices: m.indices,
        sound_nibble: m.sound_nibble,
        material_id: m.material_id,
        kind: liquid_kind(m.kind),
    }
}

/// A cell two groups both author (tile flag `0x80`) must draw once. The client picks the owner
/// from the portal flood's depth parity each frame; the two copies agree to the byte, so the
/// lower group index keeps it here.
fn resolve_shared_liquid_cells(group_liquids: &mut [Option<LiquidMesh>]) {
    let key = |m: &LiquidMesh, c: usize| -> Option<(i32, i32)> {
        let (cols, rows) = (m.grid[0] as usize, m.grid[1] as usize);
        let (xt, yt) = (cols.checked_sub(1)?, rows.checked_sub(1)?);
        let (tx, ty) = (c % xt, c / xt);
        if ty >= yt {
            return None;
        }
        let a = m.positions.get(ty * cols + tx)?;
        let b = m.positions.get((ty + 1) * cols + tx + 1)?;
        Some((
            (f32::midpoint(a[0], b[0]) * 100.0).round() as i32,
            (f32::midpoint(a[1], b[1]) * 100.0).round() as i32,
        ))
    };
    let mut owner: HashMap<(i32, i32), usize> = HashMap::new();
    for (gi, slot) in group_liquids.iter().enumerate() {
        let Some(m) = slot else { continue };
        for c in 0..m.wet.len() {
            if m.wet[c]
                && m.shared[c]
                && let Some(k) = key(m, c)
            {
                owner.entry(k).or_insert(gi);
            }
        }
    }
    if owner.is_empty() {
        return;
    }
    for (gi, slot) in group_liquids.iter_mut().enumerate() {
        let Some(m) = slot else { continue };
        let keys: Vec<Option<(i32, i32)>> = (0..m.wet.len()).map(|c| key(m, c)).collect();
        let shared = m.shared.clone();
        m.retain_cells(|c| !shared[c] || keys[c].is_none_or(|k| owner.get(&k) == Some(&gi)));
        if m.indices.is_empty() {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(x0: f32, shared_first: bool) -> LiquidMesh {
        LiquidMesh {
            grid: [3, 2],
            wet: vec![true, true],
            shared: vec![shared_first, false],
            positions: vec![
                [x0, 0.0, 1.0],
                [x0 + 4.0, 0.0, 1.0],
                [x0 + 8.0, 0.0, 1.0],
                [x0, 4.0, 1.0],
                [x0 + 4.0, 4.0, 1.0],
                [x0 + 8.0, 4.0, 1.0],
            ],
            uvs: vec![[0.0; 2]; 6],
            depths: vec![0.0; 6],
            indices: vec![0, 3, 4, 0, 4, 1, 1, 4, 5, 1, 5, 2],
            sound_nibble: 4,
            material_id: Some(0),
            kind: LiquidKind::Still,
        }
    }

    #[test]
    fn a_shared_cell_is_drawn_by_the_first_group_only() {
        let mut groups = vec![Some(sheet(0.0, true)), None, Some(sheet(0.0, true))];
        resolve_shared_liquid_cells(&mut groups);
        let first = groups[0].as_ref().expect("keeps both cells");
        assert_eq!(first.indices.len(), 12);
        let third = groups[2].as_ref().expect("keeps its own cell");
        assert_eq!(third.wet, vec![false, true]);
        assert_eq!(third.indices.len(), 6);
    }

    #[test]
    fn a_group_left_with_no_cell_has_no_liquid() {
        let mut lone = sheet(0.0, true);
        lone.wet = vec![true, false];
        lone.indices.truncate(6);
        let mut groups = vec![Some(sheet(0.0, true)), Some(lone)];
        resolve_shared_liquid_cells(&mut groups);
        assert!(groups[1].is_none());
    }
}
