use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::{AssetId, RenderAssetUsages, UntypedAssetId};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::PrimitiveTopology;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;

use super::marks::{MarkMaterial, MarkSpace};
use super::pick::{OnTheWorld, Pointed};
use super::{OnScreen, reach_on_screen};
use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::ground::terrain_wow_z_under;
use crate::light::LightBuffer;
use crate::m2::M2Model;
use crate::model::ModelSubmesh;
use crate::model_material::{ModelMaterial, ModelMaterials};
use crate::models::{Furnished, GhostBatches, GhostSpawner};
use crate::placements::Filed;
use crate::source::{m2_url, wmo_url};
use crate::stream::Streamer;
use crate::view::WorldCamera;
use crate::wmo::WmoModel;

const GHOST_ALPHA: f32 = 0.5;
const FOOTPRINT_COLOUR: Vec4 = Vec4::new(0.35, 0.85, 1.0, 1.0);
const FOOTPRINT_STEP_YD: f32 = 1.0;
const FOOTPRINT_LIFT_YD: f32 = 0.08;

/// A model shown where it would stand, see-through, with its footprint on the ground: the
/// placement it would make, as the files would hold it. With a pointer to follow, it stands on
/// the ground under that pointer while picking finds ground there.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Ghost {
    pub filed: Option<Filed>,
    pub follows: Option<PointerId>,
}

/// The ghost as it is drawn: the one last drawn until the one asked for has its model, and nothing
/// once none is asked for. A model that fails to load is drawn as nothing.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct GhostShown(pub Option<Filed>);

enum Model {
    M2(Handle<M2Model>),
    Wmo(Handle<WmoModel>),
}

#[derive(Resource, Default)]
pub(super) struct GhostDrawn {
    url: String,
    model: Option<Model>,
    entities: Vec<Entity>,
    meshes_held: Option<Arc<[Handle<Mesh>]>>,
    footprint: Option<Handle<MarkMaterial>>,
    outline: Vec<Vec3>,
    twins: HashMap<AssetId<ModelMaterial>, Handle<ModelMaterial>>,
}

pub(super) fn follow_pointer(
    mut ghost: ResMut<'_, Ghost>,
    pointed: Option<Res<'_, Pointed>>,
    hover: Option<Res<'_, HoverMap>>,
    world: Query<'_, '_, Entity, With<OnTheWorld>>,
) {
    let (Some(pointer), Some(filed), Some(pointed), Some(hover), Ok(world)) = (
        ghost.follows,
        ghost.filed.clone(),
        pointed,
        hover,
        world.single(),
    ) else {
        return;
    };
    let Some(under) = pointed
        .over_world(pointer, &hover, world)
        .and_then(|at| at.ground.as_ref())
    else {
        return;
    };
    let (rotation, scale) = (filed.rotation(), filed.scale());
    let stood = filed.stood(bevy_to_wow(under.point), rotation, scale);
    if ghost.filed.as_ref() != Some(&stood) {
        ghost.filed = Some(stood);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_ghost(
    ghost: Res<'_, Ghost>,
    mut shown: ResMut<'_, GhostShown>,
    mut drawn: ResMut<'_, GhostDrawn>,
    server: Res<'_, AssetServer>,
    models: (Res<'_, Assets<M2Model>>, Res<'_, Assets<WmoModel>>),
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    light: Option<Res<'_, LightBuffer>>,
    mut commands: Commands<'_, '_>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut cache: ResMut<'_, ModelMaterials>,
    mut furnished: ResMut<'_, Furnished>,
    mut marks: ResMut<'_, Assets<MarkMaterial>>,
) {
    if shown.0 == ghost.filed {
        return;
    }
    let (Some(light), (m2s, wmos)) = (light, models) else {
        return;
    };
    let drawn = &mut *drawn;
    let Some(filed) = &ghost.filed else {
        clear(&mut commands, drawn);
        drawn.model = None;
        drawn.url.clear();
        shown.0 = None;
        return;
    };
    let url = match filed {
        Filed::Doodad(d) => m2_url(&d.model),
        Filed::Building(w) => wmo_url(&w.model),
    };
    if drawn.url != url {
        drawn.model = Some(match filed {
            Filed::Doodad(_) => Model::M2(server.load(&url)),
            Filed::Building(_) => Model::Wmo(server.load(&url)),
        });
        drawn.url = url;
    }
    let (id, submeshes, is_wmo): (UntypedAssetId, Option<&[ModelSubmesh]>, bool) =
        match &drawn.model {
            Some(Model::M2(h)) => (
                h.id().untyped(),
                m2s.get(h).map(|m| &m.submeshes[..]),
                false,
            ),
            Some(Model::Wmo(h)) => (
                h.id().untyped(),
                wmos.get(h).map(|m| &m.submeshes[..]),
                true,
            ),
            None => return,
        };
    if submeshes.is_none() && !server.load_state(id).is_failed() {
        return;
    }
    clear(&mut commands, drawn);
    let submeshes = submeshes.unwrap_or_default();
    let at = filed.transform();
    let mut spawner = GhostSpawner {
        commands: &mut commands,
        meshes: &mut meshes,
        materials: &mut materials,
        cache: &mut cache,
        twins: &mut drawn.twins,
        furnished: &mut furnished,
        light: &light.0,
    };
    let GhostBatches {
        entities,
        meshes_held,
    } = spawner.spawn(id, submeshes, is_wmo, &at, GHOST_ALPHA);
    drawn.entities = entities;
    drawn.meshes_held = Some(meshes_held);
    drawn.outline = footprint_segments(submeshes, &at, &ground.0, &ground.1);
    if !drawn.outline.is_empty() {
        let colour = drawn
            .footprint
            .get_or_insert_with(|| {
                marks.add(MarkMaterial {
                    colour: FOOTPRINT_COLOUR,
                    space: MarkSpace::WorldDepthTested,
                })
            })
            .clone();
        let from_its_place: Vec<Vec3> = drawn.outline.iter().map(|p| *p - at.translation).collect();
        let mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, from_its_place);
        let outline = commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(colour),
            Transform::from_translation(at.translation),
            NoFrustumCulling,
        ));
        drawn.entities.push(outline.id());
    }
    shown.0 = Some(filed.clone());
}

fn clear(commands: &mut Commands<'_, '_>, drawn: &mut GhostDrawn) {
    for e in drawn.entities.drain(..) {
        commands.entity(e).try_despawn();
    }
    drawn.meshes_held = None;
    drawn.outline.clear();
}

pub(super) fn ghost_on_screen(
    drawn: Res<'_, GhostDrawn>,
    camera: Query<'_, '_, (&Camera, &GlobalTransform), With<WorldCamera>>,
    parts: Query<'_, '_, (&GlobalTransform, &Aabb)>,
    mut on_screen: ResMut<'_, OnScreen>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let models = parts
        .iter_many(&drawn.entities)
        .filter_map(|(at, bound)| reach_on_screen(camera, at, bound));
    let outline = drawn.outline.iter().filter_map(|&p| {
        let px = camera.0.world_to_viewport(camera.1, p).ok()?;
        Some(Rect::from_center_size(px, Vec2::ZERO))
    });
    let reach = models.chain(outline).reduce(|a, b| a.union(b));
    if on_screen.ghost != reach {
        on_screen.ghost = reach;
    }
}

fn footprint_segments(
    submeshes: &[ModelSubmesh],
    at: &Transform,
    streamer: &Streamer,
    adts: &Assets<AdtTile>,
) -> Vec<Vec3> {
    let Some((lo, hi)) = submeshes
        .iter()
        .filter_map(|s| s.aabb)
        .fold(None, |reach, b| {
            let (lo, hi) = (Vec3::from(b.min()), Vec3::from(b.max()));
            Some(reach.map_or((lo, hi), |(a, c): (Vec3, Vec3)| (a.min(lo), c.max(hi))))
        })
    else {
        return Vec::new();
    };
    let corners = [(lo.x, lo.z), (hi.x, lo.z), (hi.x, hi.z), (lo.x, hi.z)]
        .map(|(x, z)| at.transform_point(Vec3::new(x, 0.0, z)));
    let mut lines = Vec::new();
    for (i, &a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        let steps = (a.distance(b) / FOOTPRINT_STEP_YD).ceil().max(1.0) as u32;
        let mut last: Option<Vec3> = None;
        for k in 0..=steps {
            let p = a.lerp(b, k as f32 / steps as f32);
            let y = terrain_wow_z_under(streamer, adts, p).unwrap_or(at.translation.y);
            let point = p.with_y(y + FOOTPRINT_LIFT_YD);
            if let Some(from) = last {
                lines.extend([from, point]);
            }
            last = Some(point);
        }
    }
    lines
}
