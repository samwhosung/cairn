use std::io;
use std::sync::Arc;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::reflect::TypePath;
use model::{
    FootprintTris, WmoDoodad, WmoDoodadSet, WmoFog, WmoGroupInfo, WmoLight, parse_wmo_lights,
    parse_wmo_root, wmo_group_doodad_refs, wmo_group_footprint_tris, wmo_group_light_refs,
    wmo_group_submeshes,
};

use crate::model::ModelSubmesh;
use crate::source::MPQ_SOURCE;

mod rooms;

pub use rooms::{Bounds, WmoGroupNav, WmoRooms};
pub(crate) use rooms::{RoomsBuilder, Triangle, bounds};

/// A WMO building as the world draws it: every group's render batches, the rooms and portal graph
/// that decide which groups draw, and the doodads it places. Positions are the WMO's own space.
#[derive(Asset, TypePath, Default)]
pub struct WmoModel {
    pub submeshes: Vec<ModelSubmesh>,
    /// The group each of [`Self::submeshes`] belongs to.
    pub submesh_group: Vec<u16>,
    pub rooms: WmoRooms,
    /// Each material's diffuse colour: an indoor pool's body colour.
    pub material_diff_colors: Vec<[f32; 3]>,
    /// Per group: the render faces a down-ray reads a surface's material off; `None` unless the
    /// group is interior with vertex colours.
    pub group_footprints: Vec<Option<FootprintTris>>,
    /// Per group: the box of its footprint's faces.
    pub group_footprint_bounds: Vec<Option<Bounds>>,
    /// Per material: the `TerrainType` its surfaces are.
    pub material_ground_types: Vec<u32>,
    pub doodads: Vec<WmoDoodad>,
    pub doodad_sets: Vec<WmoDoodadSet>,
    /// Parallel to [`Self::doodads`]: how each is lit.
    pub doodad_base: Vec<DoodadBase>,
    /// Parallel to [`Self::doodads`]: every group that places it.
    pub doodad_groups: Vec<Arc<[u16]>>,
    pub lights: Vec<WmoLight>,
    /// Per group: the lights of [`Self::lights`] that light its doodads.
    pub group_light_refs: Vec<Vec<u16>>,
    /// The fogs its rooms ask for; the first is the building's own.
    pub fogs: Vec<WmoFog>,
    /// The model a room that asks for it shows as its sky.
    pub skybox: Option<String>,
}

/// How a WMO's doodad is lit.
#[derive(Clone, PartialEq, Debug)]
pub enum DoodadBase {
    /// By the sky, like any map doodad.
    Exterior,
    /// By its own baked colour and the lights of the group that first places it.
    Interior {
        ambient: [f32; 3],
        diffuse: [f32; 3],
        light_refs: Vec<u16>,
    },
}

/// The ambient word from a doodad's baked colour: scaled down so no channel passes 96, in the
/// client's fixed point.
pub(crate) fn cap96(c: [u8; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if max <= 96 {
        return c.map(|v| f32::from(v) / 255.0);
    }
    let scale = ((96.0 * 255.0 / f32::from(max)) - 0.5).round_ties_even() as u32;
    c.map(|v| ((u32::from(v) * scale + 255) >> 8) as f32 / 255.0)
}

/// The diffuse word from a doodad's baked colour: raised, keeping its hue, until its brightest
/// channel is 112, truncating.
pub(crate) fn floor112(c: [u8; 3]) -> [f32; 3] {
    floor_raise(c, 112)
}

/// The diffuse word a unit takes from the baked colour of the floor under it: as a doodad's, raised
/// to 168.
pub(crate) fn floor168(c: [u8; 3]) -> [f32; 3] {
    floor_raise(c, 168)
}

fn floor_raise(c: [u8; 3], thresh: u32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if u32::from(max) >= thresh || max == 0 {
        return c.map(|v| f32::from(v) / 255.0);
    }
    c.map(|v| ((u32::from(v) * thresh) / u32::from(max)) as f32 / 255.0)
}

fn first_placing_groups(doodad_count: usize, group_refs: &[Vec<u16>]) -> Vec<Option<u16>> {
    let mut owner = vec![None; doodad_count];
    for (gi, refs) in group_refs.iter().enumerate() {
        for &di in refs {
            if let Some(slot) = owner.get_mut(di as usize) {
                slot.get_or_insert(gi as u16);
            }
        }
    }
    owner
}

fn placing_groups(doodad_count: usize, group_refs: &[Vec<u16>]) -> Vec<Arc<[u16]>> {
    let mut refs: Vec<Vec<u16>> = vec![Vec::new(); doodad_count];
    for (gi, group) in group_refs.iter().enumerate() {
        for &di in group {
            if let Some(slot) = refs.get_mut(di as usize)
                && slot.last() != Some(&(gi as u16))
            {
                slot.push(gi as u16);
            }
        }
    }
    refs.into_iter().map(Arc::from).collect()
}

fn resolve_doodad_bases(
    doodads: &[WmoDoodad],
    groups: &[WmoGroupInfo],
    owner: &[Option<u16>],
    refs: &[Arc<[u16]>],
    group_light_refs: &[Vec<u16>],
) -> Vec<DoodadBase> {
    let interior = |gi: &u16| groups.get(*gi as usize).is_some_and(|g| g.interior);
    doodads
        .iter()
        .enumerate()
        .map(|(di, d)| {
            let Some(gi) = owner.get(di).copied().flatten() else {
                return DoodadBase::Exterior;
            };
            let all_interior = refs
                .get(di)
                .is_some_and(|gs| !gs.is_empty() && gs.iter().all(interior));
            if !all_interior {
                return DoodadBase::Exterior;
            }
            let rgb = [d.color[0], d.color[1], d.color[2]];
            DoodadBase::Interior {
                ambient: cap96(rgb),
                diffuse: floor112(rgb),
                light_refs: group_light_refs
                    .get(gi as usize)
                    .cloned()
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn footprint_bounds(fp: &FootprintTris) -> Option<Bounds> {
    bounds(
        fp.indices
            .iter()
            .filter_map(|&i| fp.positions.get(i as usize)),
    )
}

#[derive(Default, TypePath)]
pub(crate) struct WmoLoader;

impl AssetLoader for WmoLoader {
    type Asset = WmoModel;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<WmoModel, io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let root = parse_wmo_root(&bytes).map_err(io::Error::other)?;
        let path = ctx.path().path().to_string_lossy().to_ascii_lowercase();
        let stem = path.strip_suffix(".wmo").unwrap_or(&path).to_owned();
        let groups = root.group_count() as usize;
        let mut rooms = RoomsBuilder::new(&root, &bytes);
        let mut submeshes = Vec::new();
        let mut submesh_group = Vec::new();
        let mut group_doodad_refs = vec![Vec::new(); groups];
        let mut group_light_refs = vec![Vec::new(); groups];
        let mut group_footprints = vec![None; groups];
        for gi in 0..groups {
            let url = format!("{MPQ_SOURCE}://{stem}_{gi:03}.wmo");
            let Ok(gbytes) = ctx.read_asset_bytes(url).await else {
                continue;
            };
            rooms.add_group(gi, &gbytes);
            group_doodad_refs[gi] = wmo_group_doodad_refs(&gbytes);
            group_light_refs[gi] = wmo_group_light_refs(&gbytes);
            group_footprints[gi] = wmo_group_footprint_tris(&gbytes);
            for sub in wmo_group_submeshes(&gbytes, &root) {
                submeshes.push(ModelSubmesh::load(ctx, sub));
                submesh_group.push(gi as u16);
            }
        }
        let doodad_owner = first_placing_groups(root.doodads().len(), &group_doodad_refs);
        let doodad_groups = placing_groups(root.doodads().len(), &group_doodad_refs);
        let doodad_base = resolve_doodad_bases(
            root.doodads(),
            root.group_infos(),
            &doodad_owner,
            &doodad_groups,
            &group_light_refs,
        );
        let group_footprint_bounds = group_footprints
            .iter()
            .map(|fp| fp.as_ref().and_then(footprint_bounds))
            .collect();
        Ok(WmoModel {
            submeshes,
            submesh_group,
            rooms: rooms.finish(),
            material_diff_colors: root.material_diff_colors(),
            group_footprints,
            group_footprint_bounds,
            material_ground_types: root.material_ground_types(),
            doodads: root.doodads().to_vec(),
            doodad_sets: root.doodad_sets().to_vec(),
            doodad_base,
            doodad_groups,
            lights: parse_wmo_lights(&bytes),
            group_light_refs,
            fogs: root.fogs().to_vec(),
            skybox: root.skybox().map(str::to_owned),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["wmo"]
    }
}

#[cfg(test)]
impl WmoModel {
    /// A building with nothing in it, for a test to furnish.
    pub(crate) fn empty() -> Self {
        Self {
            submeshes: Vec::new(),
            submesh_group: Vec::new(),
            rooms: WmoRooms {
                wmo_id: 0,
                group_nav: Vec::new(),
                portal_vertices: Vec::new(),
                portal_infos: Vec::new(),
                portal_refs: Vec::new(),
                group_collision_tris: Vec::new(),
                group_camera_only_tris: Vec::new(),
                group_collision_bounds: Vec::new(),
                group_liquids: Vec::new(),
            },
            material_diff_colors: Vec::new(),
            group_footprints: Vec::new(),
            group_footprint_bounds: Vec::new(),
            material_ground_types: Vec::new(),
            doodads: Vec::new(),
            doodad_sets: Vec::new(),
            doodad_base: Vec::new(),
            doodad_groups: Vec::new(),
            lights: Vec::new(),
            group_light_refs: Vec::new(),
            fogs: Vec::new(),
            skybox: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(c: [f32; 3]) -> [u8; 3] {
        c.map(|v| (v * 255.0).round() as u8)
    }

    #[test]
    fn the_baked_colour_caps_and_floors_in_fixed_point() {
        assert_eq!(bytes(cap96([78, 76, 134])), [56, 55, 96]);
        assert_eq!(bytes(cap96([90, 86, 141])), [61, 59, 96]);
        assert_eq!(bytes(cap96([96, 40, 20])), [96, 40, 20]);
        assert_eq!(bytes(floor112([78, 76, 134])), [78, 76, 134]);
        assert_eq!(bytes(floor112([56, 28, 14])), [112, 56, 28]);
        assert_eq!(bytes(floor112([0, 0, 0])), [0, 0, 0]);
        assert_eq!(bytes(floor168([141, 105, 59])), [168, 125, 70]);
        assert_eq!(bytes(floor168([200, 20, 0])), [200, 20, 0]);
    }

    fn prop(color: [u8; 4]) -> WmoDoodad {
        WmoDoodad {
            model: "candle.m2".into(),
            position: [0.0; 3],
            orientation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
            color,
        }
    }

    fn group(interior: bool) -> WmoGroupInfo {
        WmoGroupInfo {
            interior,
            show_skybox: false,
            bbox_min: [-1.0; 3],
            bbox_max: [1.0; 3],
        }
    }

    #[test]
    fn one_exterior_group_makes_a_doodad_sky_lit() {
        let doodads = [prop([0, 0, 0, 255]), prop([90, 86, 141, 255])];
        let groups = [group(true), group(false), group(true)];
        let modr = vec![vec![0u16, 1], vec![0u16], vec![1u16]];
        let molr = vec![vec![7u16], Vec::new(), vec![4u16]];
        let owner = first_placing_groups(doodads.len(), &modr);
        let refs = placing_groups(doodads.len(), &modr);
        assert_eq!(owner, [Some(0), Some(0)]);
        let bases = resolve_doodad_bases(&doodads, &groups, &owner, &refs, &molr);
        assert_eq!(bases[0], DoodadBase::Exterior);
        let DoodadBase::Interior {
            ambient,
            light_refs,
            ..
        } = &bases[1]
        else {
            panic!("an interior-only doodad keeps its baked colour");
        };
        assert_eq!(bytes(*ambient), [61, 59, 96]);
        assert_eq!(light_refs, &[7]);
    }

    #[test]
    fn every_placing_group_is_a_referrer_once() {
        let modr = vec![vec![2u16], vec![0u16, 2, 2], vec![1u16, 99]];
        let refs = placing_groups(3, &modr);
        assert_eq!(&*refs[0], &[1]);
        assert_eq!(&*refs[1], &[2]);
        assert_eq!(&*refs[2], &[0, 1]);
        assert_eq!(first_placing_groups(3, &modr), [Some(1), Some(2), Some(0)]);
    }
}
