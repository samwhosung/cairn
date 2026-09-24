//! What a body stands on: in a building, the material of the render face under its feet; outside,
//! the ground effect of the terrain layer under them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::ground::{Ground, ground_under};
use crate::interior::{FEET_PROBE_LIFT, WmoRoom};
use crate::portal::WmoPortalInstance;
use crate::stream::Streamer;
use crate::wmo::WmoModel;

/// The surface a footfall lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underfoot {
    /// A building's face: its `TerrainType` id.
    Terrain(u32),
    /// The terrain: its `GroundEffectTexture` id.
    GroundEffect(u32),
}

#[derive(SystemParam)]
pub struct SurfaceUnderfoot<'w, 's> {
    wmos: Res<'w, Assets<WmoModel>>,
    instances: Query<'w, 's, &'static WmoPortalInstance>,
    streamer: Res<'w, Streamer>,
    adts: Res<'w, Assets<AdtTile>>,
}

impl SurfaceUnderfoot<'_, '_> {
    /// Under `feet` (Bevy space) for a body in `room`. `None` is silent: a building that owns the
    /// column but has no face under the feet never falls back to the ground below its floor.
    pub fn at(&self, room: Option<WmoRoom>, feet: Vec3) -> Option<Underfoot> {
        match room {
            Some(room) => {
                let inst = self.instances.get(room.instance).ok()?;
                let model = self.wmos.get(&inst.handle)?;
                let probe = feet + Vec3::Y * FEET_PROBE_LIFT;
                let local = inst.world_from_local.inverse().transform_point3(probe);
                material_under(model, usize::from(room.group), bevy_to_wow(local))
                    .map(Underfoot::Terrain)
            }
            None => match ground_under(&self.streamer, &self.adts, feet) {
                Ground::Tile(adt) => terrain::ground_effect_at(&adt.chunks, bevy_to_wow(feet))
                    .map(Underfoot::GroundEffect),
                _ => None,
            },
        }
    }
}

fn material_under(model: &WmoModel, group: usize, probe: [f32; 3]) -> Option<u32> {
    let fp = model.group_footprints.get(group)?.as_ref()?;
    let [px, py, pz] = probe;
    if let Some(Some((min, max))) = model.group_footprint_bounds.get(group)
        && (px < min[0] || px > max[0] || py < min[1] || py > max[1] || min[2] > pz)
    {
        return None;
    }
    let mut best: Option<(f32, u8)> = None;
    for (ti, tri) in fp.indices.as_chunks::<3>().0.iter().enumerate() {
        let (Some(a), Some(b), Some(c)) = (
            fp.positions.get(usize::from(tri[0])),
            fp.positions.get(usize::from(tri[1])),
            fp.positions.get(usize::from(tri[2])),
        ) else {
            continue;
        };
        let det = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if det.abs() < 1e-9 {
            continue;
        }
        let wb = ((px - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (py - a[1])) / det;
        let wc = ((b[0] - a[0]) * (py - a[1]) - (px - a[0]) * (b[1] - a[1])) / det;
        let wa = 1.0 - wb - wc;
        if wa < 0.0 || wb < 0.0 || wc < 0.0 {
            continue;
        }
        let z = wa * a[2] + wb * b[2] + wc * c[2];
        if z > pz || best.is_some_and(|(bz, _)| z < bz) {
            continue;
        }
        best = Some((z, fp.mopy_material.get(ti).copied().unwrap_or(0xFF)));
    }
    let (_, material) = best?;
    model
        .material_ground_types
        .get(usize::from(material))
        .copied()
}
