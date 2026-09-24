use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::VisibilitySystems;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use model::{ModelBlend, RenderSubmesh};

use crate::coords::wow_to_bevy;
use crate::light::LightBuffer;
use crate::m2::M2Model;
use crate::model_material::{BatchLook, GroundShade, ModelMaterial, ModelMaterials, Variant};
use crate::portal::{WmoPortalInstance, compute_wmo_pvs};
use crate::room::RoomCrossfade;
use crate::submersion::Underwater;
use crate::view::WorldCamera;
use crate::visibility::alpha_bits;
use crate::wmo::WmoModel;
use crate::{Residency, m2_url};

const GROUP_SHOWS_SKYBOX: u32 = 0x40000;
const REPLACES_SKY: f32 = 0.99;

#[derive(Component)]
pub(crate) struct SkyPass;

#[derive(Resource, Default, Clone, PartialEq, Debug)]
pub(crate) struct Skybox {
    wanted: Option<String>,
    weight: f32,
}

#[derive(Component)]
struct SkyboxPart {
    path: String,
    steady: Handle<ModelMaterial>,
    fade: Handle<ModelMaterial>,
}

enum Build {
    Loading(Handle<M2Model>),
    Built,
}

#[derive(Resource, Default)]
struct Built(HashMap<String, Build>);

pub(crate) struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Skybox>()
            .init_resource::<Built>()
            .add_systems(
                Update,
                (resolve, build, show, stand_down_sky)
                    .chain()
                    .after(compute_wmo_pvs)
                    .after(crate::atmosphere::resolve_light),
            )
            .add_systems(
                PostUpdate,
                follow_camera
                    .after(TransformSystems::Propagate)
                    .before(VisibilitySystems::CheckVisibility),
            );
    }
}

fn resolve(
    instances: Query<'_, '_, &WmoPortalInstance>,
    wmos: Res<'_, Assets<WmoModel>>,
    crossfade: Res<'_, RoomCrossfade>,
    mut skybox: ResMut<'_, Skybox>,
) {
    let wanted = instances
        .iter()
        .filter_map(|inst| {
            let model = wmos.get(&inst.handle)?;
            let sky = model.skybox.as_deref()?;
            let mut groups = model.rooms.group_nav.iter().zip(&inst.visible);
            groups
                .any(|(nav, &visible)| visible && nav.flags & GROUP_SHOWS_SKYBOX != 0)
                .then(|| sky.to_owned())
        })
        .min();
    let weight = if wanted.is_some() {
        crossfade.weight()
    } else {
        0.0
    };
    let resolved = Skybox { wanted, weight };
    if *skybox != resolved {
        *skybox = resolved;
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    mut commands: Commands<'_, '_>,
    skybox: Res<'_, Skybox>,
    server: Res<'_, AssetServer>,
    models: Res<'_, Assets<M2Model>>,
    light: Option<Res<'_, LightBuffer>>,
    mut built: ResMut<'_, Built>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut cache: ResMut<'_, ModelMaterials>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut residency: ResMut<'_, Residency>,
) {
    let Some(path) = skybox.wanted.as_deref() else {
        residency.skybox_pending = false;
        return;
    };
    let (Some(light), Some(state)) = (light, built.0.get(path)) else {
        built
            .0
            .entry(path.to_owned())
            .or_insert_with(|| Build::Loading(server.load(m2_url(path))));
        residency.skybox_pending = true;
        return;
    };
    let Build::Loading(handle) = state else {
        residency.skybox_pending = false;
        return;
    };
    if server.load_state(handle).is_failed() {
        warn!("no painted sky: {path} did not load");
    } else if !server.is_loaded_with_dependencies(handle) {
        residency.skybox_pending = true;
        return;
    } else if let Some(model) = models.get(handle) {
        for (i, sub) in model.submeshes.iter().enumerate() {
            let order = u16::try_from(i + 1).unwrap_or(u16::MAX);
            let look = sky_look(&sub.geometry, sub.texture.clone(), order);
            let steady = cache.get(&mut materials, &look, Variant::Steady, &light.0);
            let blends = matches!(
                look.blend,
                ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
            );
            let fade = if blends {
                steady.clone()
            } else {
                cache.get(&mut materials, &look, Variant::FadeTwin, &light.0)
            };
            commands.spawn((
                Mesh3d(meshes.add(sky_mesh(&sub.geometry))),
                MeshMaterial3d(steady.clone()),
                Transform::default(),
                Visibility::Hidden,
                MeshTag(alpha_bits(1.0)),
                SkyboxPart {
                    path: path.to_owned(),
                    steady,
                    fade,
                },
            ));
        }
    }
    built.0.insert(path.to_owned(), Build::Built);
    residency.skybox_pending = false;
}

fn sky_look(g: &RenderSubmesh, texture: Option<Handle<Image>>, order: u16) -> BatchLook {
    BatchLook {
        texture,
        blend: g.blend,
        two_sided: g.two_sided,
        is_wmo: false,
        interior: false,
        emissive: g.emissive,
        additive: g.additive,
        no_depth_write: true,
        no_depth_test: false,
        fog_policy: g.fog_policy,
        env_map: g.env_map,
        shade: GroundShade::Lit,
        batch_order: std::num::NonZeroU16::new(order),
        uv_offset_at_rest: [0.0, 0.0],
        tint_at_rest: [1.0; 3],
        animated: None,
        seq_owner: None,
        wmo_class: None,
        sidn: None,
        window: false,
        skybox: true,
    }
}

fn sky_mesh(g: &RenderSubmesh) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let bevy = |v: &[[f32; 3]]| -> Vec<[f32; 3]> {
        v.iter().map(|p| wow_to_bevy(*p).to_array()).collect()
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, bevy(&g.positions));
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, g.uvs.clone());
    mesh.insert_indices(Indices::U32(g.indices.clone()));
    if g.normals.len() == g.positions.len() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, bevy(&g.normals));
    } else {
        mesh.compute_normals();
    }
    mesh
}

fn show(
    skybox: Res<'_, Skybox>,
    mut parts: Query<
        '_,
        '_,
        (
            &SkyboxPart,
            &mut Visibility,
            &mut MeshMaterial3d<ModelMaterial>,
            &mut MeshTag,
        ),
    >,
) {
    for (part, mut visibility, mut material, mut tag) in &mut parts {
        let shown = skybox.weight > 0.0 && skybox.wanted.as_deref() == Some(part.path.as_str());
        let want = if shown {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(want);
        if !shown {
            continue;
        }
        let handle = if skybox.weight < 1.0 {
            &part.fade
        } else {
            &part.steady
        };
        if material.0 != *handle {
            material.0 = handle.clone();
        }
        tag.set_if_neq(MeshTag(alpha_bits(skybox.weight)));
    }
}

fn stand_down_sky(
    skybox: Res<'_, Skybox>,
    underwater: Res<'_, Underwater>,
    mut sky: Query<'_, '_, &mut Visibility, With<SkyPass>>,
) {
    let want = if skybox.weight > REPLACES_SKY || underwater.0.any() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut visibility in &mut sky {
        visibility.set_if_neq(want);
    }
}

type PartTransforms<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut GlobalTransform),
    (With<SkyboxPart>, Without<WorldCamera>),
>;

fn follow_camera(
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    mut parts: PartTransforms<'_, '_>,
) {
    let Ok(eye) = camera.single() else {
        return;
    };
    for (mut transform, mut global) in &mut parts {
        *transform = Transform::from_translation(eye.translation());
        *global = GlobalTransform::from(*transform);
    }
}
