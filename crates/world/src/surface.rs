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
    let hit = footprint_under(model, probe, Some(group))?;
    model
        .material_ground_types
        .get(usize::from(hit.material))
        .copied()
}

const MOPY_LIT_BY_DAY_NIGHT: u8 = 0x1;

pub(crate) struct FootprintHit {
    pub group: usize,
    pub mocv_at_hit: [u8; 3],
    pub lit_by_day_night: bool,
    pub material: u8,
}

/// Of faces at the same height the later one wins, as in the client.
pub(crate) fn footprint_under(
    model: &WmoModel,
    probe: [f32; 3],
    only_group: Option<usize>,
) -> Option<FootprintHit> {
    let [px, py, pz] = probe;
    let mut best: Option<(f32, FootprintHit)> = None;
    for (gi, fp) in model.group_footprints.iter().enumerate() {
        let Some(fp) = fp else { continue };
        if only_group.is_some_and(|g| g != gi) {
            continue;
        }
        if let Some(Some((min, max))) = model.group_footprint_bounds.get(gi)
            && (px < min[0] || px > max[0] || py < min[1] || py > max[1] || min[2] > pz)
        {
            continue;
        }
        for (ti, tri) in fp.indices.as_chunks::<3>().0.iter().enumerate() {
            let corner = |k: usize| {
                let i = usize::from(tri[k]);
                Some((*fp.positions.get(i)?, *fp.mocv.get(i)?))
            };
            let (Some((a, ca)), Some((b, cb)), Some((c, cc))) = (corner(0), corner(1), corner(2))
            else {
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
            if z > pz || best.as_ref().is_some_and(|(bz, _)| z < *bz) {
                continue;
            }
            let mocv_at_hit = std::array::from_fn(|k| {
                let v = wa * f32::from(ca[k]) + wb * f32::from(cb[k]) + wc * f32::from(cc[k]);
                v.round().clamp(0.0, 255.0) as u8
            });
            let hit = FootprintHit {
                group: gi,
                mocv_at_hit,
                lit_by_day_night: fp
                    .mopy_flags
                    .get(ti)
                    .is_some_and(|f| f & MOPY_LIT_BY_DAY_NIGHT != 0),
                material: fp.mopy_material.get(ti).copied().unwrap_or(0xFF),
            };
            best = Some((z, hit));
        }
    }
    best.map(|(_, hit)| hit)
}

#[cfg(test)]
mod tests {
    use model::FootprintTris;

    use super::*;

    fn floor(z: f32, mocv: [[u8; 3]; 3], flags: u8) -> FootprintTris {
        FootprintTris {
            positions: vec![[0.0, 0.0, z], [10.0, 0.0, z], [0.0, 10.0, z]],
            indices: vec![0, 1, 2],
            mocv: mocv.to_vec(),
            mopy_flags: vec![flags],
            mopy_material: vec![3],
        }
    }

    #[test]
    fn the_nearest_floor_below_gives_its_colour_where_the_ray_meets_it() {
        let mut m = WmoModel::empty();
        m.group_footprints = vec![
            Some(floor(0.0, [[0, 0, 0], [200, 100, 50], [0, 0, 0]], 0)),
            Some(floor(2.0, [[9; 3]; 3], MOPY_LIT_BY_DAY_NIGHT)),
        ];
        m.group_footprint_bounds = vec![None, None];
        let hit = footprint_under(&m, [5.0, 0.0, 1.0], None).expect("the lower floor");
        assert_eq!(
            (hit.group, hit.mocv_at_hit, hit.lit_by_day_night),
            (0, [100, 50, 25], false)
        );
        let hit = footprint_under(&m, [5.0, 0.0, 3.0], None).expect("the upper floor");
        assert_eq!(
            (hit.group, hit.lit_by_day_night, hit.material),
            (1, true, 3)
        );
        let hit = footprint_under(&m, [5.0, 0.0, 3.0], Some(0)).expect("the one group asked");
        assert_eq!(hit.group, 0);
        assert!(footprint_under(&m, [8.0, 8.0, 3.0], None).is_none());
    }
}
