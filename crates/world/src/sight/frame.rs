//! Which placement each pixel of a frame shows: every model batch and the ground drawn again for a
//! sight camera on [`SIGHT_LAYER`], each in the colour of its placement's index.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::view::{Msaa, NoIndirectDrawing};

use super::{Meetable, Seen};
use crate::LeftOut;
use crate::model_material::{ModelMaterial, sight_twin_of};
use crate::terrain::{self, TerrainMaterial};
use crate::view::{FOV_Y, NEARCLIP, PROJECTION_FAR};
use crate::visibility::{SIGHT_INDICES, sight_tag};

/// The render layer the sight camera and the batches' sight twins share.
pub const SIGHT_LAYER: usize = 31;
pub(crate) const SIGHT_GROUND: u32 = 1;
const FIRST_PLACEMENT: u32 = 2;

/// Keeps a sight twin beside every model batch and every terrain tile, which follows its batch
/// while [`SightWanted`] is set.
pub struct SightFramePlugin;

impl Plugin for SightFramePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SightWanted>()
            .init_resource::<SightIndex>()
            .init_resource::<SightMaterials>()
            .add_systems(Last, (twin_new_batches, follow_batches).chain());
    }
}

/// Whether a sight camera draws: while it does, each twin takes its batch's mesh, material and
/// tag every frame.
#[derive(Resource, Default)]
pub struct SightWanted(pub bool);

/// A placement a sight frame names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub unique_id: u32,
    pub building: bool,
    /// Its path in the install.
    pub file: Arc<str>,
}

/// What a pixel of a sight frame shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown<'a> {
    /// Nothing within the far clip: the sky, the horizon or a liquid's surface with nothing under
    /// it.
    Nothing,
    Ground,
    Placed(&'a Placed),
}

/// The placements the twins' colours name, by index.
#[derive(Resource, Default)]
pub struct SightIndex(Vec<Placed>);

impl SightIndex {
    /// Placements by index in the order of their unique ids, the first after the ground's.
    pub fn of(mut placements: Vec<Placed>) -> Self {
        placements.sort_by_key(|p| p.unique_id);
        Self(placements)
    }

    /// The sRGB bytes a sight frame gives the pixels a placement covers.
    pub fn colour(&self, unique_id: u32) -> Option<[u8; 3]> {
        let at = self
            .0
            .binary_search_by_key(&unique_id, |p| p.unique_id)
            .ok()?;
        Some(sight_colour(FIRST_PLACEMENT + at as u32))
    }

    /// The sRGB bytes a sight frame gives the ground.
    pub fn ground_colour() -> [u8; 3] {
        sight_colour(SIGHT_GROUND)
    }

    /// What the pixel of a sight frame whose sRGB bytes are `rgb` shows.
    pub fn shown(&self, rgb: [u8; 3]) -> Option<Shown<'_>> {
        let [a, b, c] = rgb.map(|byte| u32::from(byte) / 4);
        match a | b << 6 | c << 12 {
            0 => Some(Shown::Nothing),
            SIGHT_GROUND => Some(Shown::Ground),
            index => self
                .0
                .get((index - FIRST_PLACEMENT) as usize)
                .map(Shown::Placed),
        }
    }
}

fn sight_colour(index: u32) -> [u8; 3] {
    [index & 63, (index >> 6) & 63, index >> 12].map(|code| (code * 4 + 2) as u8)
}

#[derive(Resource, Default)]
struct SightMaterials {
    twins: HashMap<AssetId<ModelMaterial>, Handle<ModelMaterial>>,
}

#[derive(Component)]
struct SightTwin;

#[derive(Component)]
struct Twinned;

/// A camera drawing the sight frame: place it as the world camera stands, with a target of the
/// world camera's size, and make it active for the frames it should draw.
pub fn sight_camera(transform: Transform, target: Handle<Image>) -> impl Bundle {
    (
        Camera3d::default(),
        Camera {
            order: 1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..Camera::default()
        },
        Projection::from(PerspectiveProjection {
            fov: FOV_Y,
            near: NEARCLIP,
            far: PROJECTION_FAR,
            ..PerspectiveProjection::default()
        }),
        Tonemapping::None,
        DebandDither::Disabled,
        Msaa::Off,
        NoIndirectDrawing,
        RenderLayers::layer(SIGHT_LAYER),
        RenderTarget::Image(target.into()),
        transform,
    )
}

type NewBatch<'a> = (
    Entity,
    &'a Mesh3d,
    &'a MeshTag,
    Option<&'a MeshMaterial3d<ModelMaterial>>,
);

type NewTile<'a> = (Entity, &'a Mesh3d, &'a MeshMaterial3d<TerrainMaterial>);

#[allow(clippy::type_complexity)]
fn twin_new_batches(
    mut commands: Commands<'_, '_>,
    batches: Query<'_, '_, NewBatch<'_>, (With<Meetable>, Without<Twinned>)>,
    tiles: Query<'_, '_, NewTile<'_>, (Without<Twinned>, Without<SightTwin>)>,
    mut store: ResMut<'_, SightMaterials>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut grounds: ResMut<'_, Assets<TerrainMaterial>>,
) {
    for (batch, mesh, tag, material) in &batches {
        let mut twin = commands.spawn((
            SightTwin,
            mesh.clone(),
            MeshTag(tag.0),
            RenderLayers::layer(SIGHT_LAYER),
            NoFrustumCulling,
            ChildOf(batch),
        ));
        if let Some(material) = material.and_then(|m| store.twin(&mut materials, m.id())) {
            twin.insert(MeshMaterial3d(material));
        }
        commands.entity(batch).insert(Twinned);
    }
    for (tile, mesh, drawn) in &tiles {
        let Some(ground) = grounds.get(drawn).map(terrain::sight_twin_of) else {
            continue;
        };
        commands.spawn((
            SightTwin,
            mesh.clone(),
            MeshMaterial3d(grounds.add(ground)),
            RenderLayers::layer(SIGHT_LAYER),
            NoFrustumCulling,
            ChildOf(tile),
        ));
        commands.entity(tile).insert(Twinned);
    }
}

impl SightMaterials {
    fn twin(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        drawn: AssetId<ModelMaterial>,
    ) -> Option<Handle<ModelMaterial>> {
        let slots = materials.get(drawn)?.extension.anim_slots;
        if let Some(handle) = self.twins.get(&drawn) {
            let current = materials
                .get(handle)
                .is_some_and(|m| m.extension.anim_slots == slots);
            if !current {
                let twin = sight_twin_of(materials.get(drawn)?);
                materials.insert(handle.id(), twin).ok()?;
            }
            return Some(handle.clone());
        }
        let handle = materials.add(sight_twin_of(materials.get(drawn)?));
        self.twins.insert(drawn, handle.clone());
        Some(handle)
    }
}

type Batch<'a> = (
    &'a Mesh3d,
    &'a MeshTag,
    &'a MeshMaterial3d<ModelMaterial>,
    &'a Meetable,
);

type Twin<'a> = (
    &'a ChildOf,
    &'a mut Mesh3d,
    &'a mut MeshTag,
    &'a mut Visibility,
    Option<&'a mut MeshMaterial3d<ModelMaterial>>,
);

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn follow_batches(
    wanted: Res<'_, SightWanted>,
    mut commands: Commands<'_, '_>,
    batches: Query<'_, '_, Batch<'_>, Without<SightTwin>>,
    mut twins: Query<'_, '_, (Entity, Twin<'_>), With<SightTwin>>,
    mut store: ResMut<'_, SightMaterials>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut index: ResMut<'_, SightIndex>,
    left_out: Res<'_, LeftOut>,
) {
    if !wanted.0 {
        return;
    }
    let mut placed: BTreeMap<u32, Placed> = BTreeMap::new();
    for (_, _, _, met) in &batches {
        if let Some(p) = placement(&met.seen) {
            placed.entry(p.unique_id).or_insert(p);
        }
    }
    if placed.len() > (SIGHT_INDICES - FIRST_PLACEMENT) as usize {
        warn_once!(
            "a sight frame names {} placements, more than it can tell apart",
            placed.len()
        );
    }
    let index_of: BTreeMap<u32, u32> = placed
        .keys()
        .zip(FIRST_PLACEMENT..)
        .map(|(&id, i)| (id, i))
        .collect();
    for (twin, (parent, mut mesh, mut tag, mut visibility, material)) in &mut twins {
        let Ok((drawn_mesh, drawn_tag, drawn_material, met)) = batches.get(parent.parent()) else {
            continue;
        };
        let Some(id) = met.seen.placement() else {
            continue;
        };
        let shown = if left_out.0.contains(&id) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != shown {
            *visibility = shown;
        }
        let Some(&i) = index_of.get(&id) else {
            continue;
        };
        if mesh.0 != drawn_mesh.0 {
            mesh.0 = drawn_mesh.0.clone();
        }
        let sighted = MeshTag(sight_tag(drawn_tag.0, i));
        if *tag != sighted {
            *tag = sighted;
        }
        let Some(want) = store.twin(&mut materials, drawn_material.id()) else {
            continue;
        };
        match material {
            Some(mut m) if m.0 != want => m.0 = want,
            Some(_) => {}
            None => {
                commands.entity(twin).insert(MeshMaterial3d(want));
            }
        }
    }
    index.0 = placed.into_values().collect();
}

fn placement(seen: &Seen) -> Option<Placed> {
    let (unique_id, building, file) = match seen {
        Seen::Terrain { .. } => return None,
        Seen::Doodad { file, unique_id } => (*unique_id, false, file),
        Seen::Building {
            file, unique_id, ..
        } => (*unique_id, true, file),
        Seen::Prop {
            building_file,
            building_unique_id,
            ..
        } => (*building_unique_id, true, building_file),
    };
    Some(Placed {
        unique_id,
        building,
        file: file.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(unique_id: u32) -> Placed {
        Placed {
            unique_id,
            building: false,
            file: Arc::from("world/x.m2"),
        }
    }

    fn rounded_by(off: i32, index: u32) -> [u8; 3] {
        sight_colour(index).map(|byte| (i32::from(byte) + off) as u8)
    }

    #[test]
    fn a_pixel_names_the_sky_the_ground_or_its_placement_whatever_a_rounding_did() {
        let index = SightIndex::of((0..5000).map(placed).collect());
        for off in [-1, 0, 1] {
            assert_eq!(index.shown(rounded_by(off, 0)), Some(Shown::Nothing));
            assert_eq!(
                index.shown(rounded_by(off, SIGHT_GROUND)),
                Some(Shown::Ground)
            );
            for i in [0u32, 1, 63, 64, 4095, 4096, 4999] {
                let at = rounded_by(off, i + FIRST_PLACEMENT);
                assert_eq!(
                    index.shown(at),
                    Some(Shown::Placed(&placed(i))),
                    "{i} {off}"
                );
            }
        }
        assert_eq!(index.shown(rounded_by(0, 5000 + FIRST_PLACEMENT)), None);
        assert_eq!(
            index.colour(4096),
            Some(sight_colour(4096 + FIRST_PLACEMENT))
        );
        assert_eq!(index.colour(5000), None);
    }
}
