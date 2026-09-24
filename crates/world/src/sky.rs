use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::Projection;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use bevy::transform::TransformSystems;

use crate::light::SceneLight;
use crate::sky_order::{SKY_VERTEX_SHADER, sky_pipeline_state};
use crate::skybox::ReplacedByPaintedSky;
use crate::submersion::{SubmersionVerdict, Underwater};
use crate::view::WorldCamera;

pub type SkyMaterial = ExtendedMaterial<StandardMaterial, SkyExtension>;

/// The dome's radius as a fraction of the projection's far plane, so it is never clipped.
const DOME_SCALE_OF_FAR: f32 = 0.9;

#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct SkyExtension {
    #[uniform(100)]
    sky0: Vec4,
    #[uniform(100)]
    sky1: Vec4,
    #[uniform(100)]
    sky2: Vec4,
    #[uniform(100)]
    sky3: Vec4,
    #[uniform(100)]
    sky4: Vec4,
    #[uniform(100)]
    fog: Vec4,
    #[uniform(100)]
    warp: Vec4,
}

impl MaterialExtension for SkyExtension {
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/sky.wgsl".into()
    }

    /// The client draws its sky without writing depth.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        sky_pipeline_state(descriptor);
        Ok(())
    }
}

#[derive(Component)]
struct Dome;

pub(crate) struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "sky.wgsl");
        embedded_asset!(app, "sky_vertex.wgsl");
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_systems(Startup, spawn_dome)
            .add_systems(Update, hide_when_submerged.after(SubmersionVerdict))
            .add_systems(
                PostUpdate,
                (follow_camera.after(TransformSystems::Propagate), paint_dome),
            );
    }
}

fn hide_when_submerged(
    underwater: Res<'_, Underwater>,
    mut dome: Query<'_, '_, &mut Visibility, With<Dome>>,
) {
    let want = if underwater.0.any() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut vis in &mut dome {
        if *vis != want {
            *vis = want;
        }
    }
}

fn spawn_dome(
    mut commands: Commands<'_, '_>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<SkyMaterial>>,
) {
    let material = materials.add(SkyMaterial {
        base: StandardMaterial {
            unlit: true,
            cull_mode: None,
            ..StandardMaterial::default()
        },
        extension: SkyExtension {
            sky0: Vec4::ZERO,
            sky1: Vec4::ZERO,
            sky2: Vec4::ZERO,
            sky3: Vec4::ZERO,
            sky4: Vec4::ZERO,
            fog: Vec4::ZERO,
            warp: Vec4::ZERO,
        },
    });
    commands.spawn((
        Mesh3d(meshes.add(dome_mesh())),
        MeshMaterial3d(material),
        Transform::default(),
        Dome,
        ReplacedByPaintedSky,
    ));
}

fn dome_mesh() -> Mesh {
    const STACKS: usize = 24;
    const SECTORS: usize = 48;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    for i in 0..=STACKS {
        let v = i as f32 / STACKS as f32;
        let (sp, cp) = (v * std::f32::consts::PI).sin_cos();
        for j in 0..=SECTORS {
            let u = j as f32 / SECTORS as f32;
            let (st, ct) = (u * std::f32::consts::TAU).sin_cos();
            let p = [sp * ct, cp, sp * st];
            positions.push(p);
            normals.push([-p[0], -p[1], -p[2]]);
            uvs.push([u, v]);
        }
    }
    let stride = SECTORS + 1;
    let mut indices: Vec<u32> = Vec::new();
    for i in 0..STACKS {
        for j in 0..SECTORS {
            let a = (i * stride + j) as u32;
            let (b, c) = (a + 1, a + stride as u32);
            indices.extend_from_slice(&[a, b, c, b, c + 1, c]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

type DomeTransforms<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut GlobalTransform),
    (With<Dome>, Without<WorldCamera>),
>;

fn follow_camera(
    camera: Query<'_, '_, (&GlobalTransform, &Projection), With<WorldCamera>>,
    mut dome: DomeTransforms<'_, '_>,
) {
    let (Ok((eye, projection)), Ok((mut transform, mut global))) =
        (camera.single(), dome.single_mut())
    else {
        return;
    };
    let far = match projection {
        Projection::Perspective(p) => p.far,
        _ => crate::view::PROJECTION_FAR,
    };
    let radius = far * DOME_SCALE_OF_FAR;
    *transform = Transform::from_translation(eye.translation()).with_scale(Vec3::splat(radius));
    *global = GlobalTransform::from(*transform);
}

/// Colours quantised to bytes, and the warp to 1/4096, as the client's sky colours are.
fn paint_dome(
    light: Res<'_, SceneLight>,
    dome: Query<'_, '_, &MeshMaterial3d<SkyMaterial>, With<Dome>>,
    mut materials: ResMut<'_, Assets<SkyMaterial>>,
) {
    let Ok(handle) = dome.single() else {
        return;
    };
    let quantize = |x: f32, n: f32| (x * n).round() / n;
    let byte = |c: [f32; 3], w: f32| {
        Vec4::new(
            quantize(c[0], 255.0),
            quantize(c[1], 255.0),
            quantize(c[2], 255.0),
            w,
        )
    };
    let sky = light.sky.map(|c| byte(c, 1.0));
    let fog = byte(light.fog_color, 0.0);
    let sun = light.visible_sun;
    let sun_azimuth = sun.z.atan2(sun.x);
    let warp = Vec4::new(
        quantize(light.sky_warp, 4096.0),
        quantize(sun_azimuth, 4096.0),
        0.0,
        0.0,
    );
    let Some(current) = materials.get(&handle.0) else {
        return;
    };
    let e = &current.extension;
    if [e.sky0, e.sky1, e.sky2, e.sky3, e.sky4, e.fog, e.warp]
        == [sky[0], sky[1], sky[2], sky[3], sky[4], fog, warp]
    {
        return;
    }
    if let Some(material) = materials.get_mut(&handle.0) {
        let e = &mut material.extension;
        [e.sky0, e.sky1, e.sky2, e.sky3, e.sky4] = sky;
        e.fog = fog;
        e.warp = warp;
    }
}
