use bevy::asset::RenderAssetUsages;
use bevy::camera::Projection;
use bevy::image::Image;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;

use super::kernel::SIDE;
use crate::sky_order::{self, SKY_VERTEX_SHADER, sky_pipeline_state};
use crate::skybox::SkyPass;
use crate::view::WorldCamera;

pub type CloudMaterial = ExtendedMaterial<StandardMaterial, CloudExtension>;

const LAYER_SCALE_OF_FAR: f32 = 0.87;

#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct CloudExtension {
    #[texture(100)]
    #[sampler(101)]
    field_bytes: Handle<Image>,
}

impl MaterialExtension for CloudExtension {
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/clouds/cloud.wgsl".into()
    }

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
pub(super) struct CloudLayer;

#[derive(Resource)]
pub(super) struct CloudImage {
    pub image: Handle<Image>,
    pub material: Handle<CloudMaterial>,
}

const RING_COLAT_HALF_TURNS: [f32; 12] = [
    0.0, 0.025, 0.05, 0.075, 0.10, 0.125, 0.15, 0.175, 0.205, 0.23, 0.245, 0.25,
];
#[rustfmt::skip]
const RING_ALPHA: [f32; 12] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 128.0 / 255.0, 0.0, 0.0,
];
const AZ_STEPS: usize = 16;

fn layer_mesh() -> Mesh {
    let rim_colat = RING_COLAT_HALF_TURNS[RING_COLAT_HALF_TURNS.len() - 1];
    let rim_height = (rim_colat * std::f32::consts::PI).cos();
    let mut positions = Vec::with_capacity(RING_COLAT_HALF_TURNS.len() * AZ_STEPS);
    let mut uvs = Vec::with_capacity(positions.capacity());
    let mut colors = Vec::with_capacity(positions.capacity());
    for (ring, (&colat, &alpha)) in RING_COLAT_HALF_TURNS.iter().zip(&RING_ALPHA).enumerate() {
        let phi = colat * std::f32::consts::PI;
        let radius = ring as f32 / 24.0;
        for j in 0..AZ_STEPS {
            let (sa, ca) = (j as f32 / AZ_STEPS as f32 * std::f32::consts::TAU).sin_cos();
            let dir = Vec3::new(phi.sin() * sa, phi.cos() - rim_height, phi.sin() * ca).normalize();
            positions.push(dir.to_array());
            uvs.push([sa * radius + 0.5, ca * radius + 0.5]);
            colors.push([1.0, 1.0, 1.0, alpha]);
        }
    }
    let normals = positions.clone();
    let steps = AZ_STEPS as u32;
    let mut indices = Vec::with_capacity((RING_COLAT_HALF_TURNS.len() - 1) * AZ_STEPS * 6);
    for ring in 0..RING_COLAT_HALF_TURNS.len() as u32 - 1 {
        for j in 0..steps {
            let next = (j + 1) % steps;
            let (a, c) = (ring * steps + j, ring * steps + next);
            let (b, d) = (a + steps, c + steps);
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

pub(super) fn spawn_layer(
    mut commands: Commands<'_, '_>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<CloudMaterial>>,
    mut images: ResMut<'_, Assets<Image>>,
) {
    let side = SIDE as u32;
    let image = images.add(Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; SIDE * SIDE * 4],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    ));
    let material = materials.add(CloudMaterial {
        base: StandardMaterial {
            unlit: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Premultiplied,
            depth_bias: sky_order::CLOUDS_SORT_RUNG,
            ..StandardMaterial::default()
        },
        extension: CloudExtension {
            field_bytes: image.clone(),
        },
    });
    commands.spawn((
        Mesh3d(meshes.add(layer_mesh())),
        MeshMaterial3d(material.clone()),
        Transform::default(),
        CloudLayer,
        SkyPass,
    ));
    commands.insert_resource(CloudImage { image, material });
}

type LayerTransforms<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut GlobalTransform),
    (With<CloudLayer>, Without<WorldCamera>),
>;

pub(super) fn follow_camera(
    camera: Query<'_, '_, (&GlobalTransform, &Projection), With<WorldCamera>>,
    mut layer: LayerTransforms<'_, '_>,
) {
    let (Ok((eye, projection)), Ok((mut transform, mut global))) =
        (camera.single(), layer.single_mut())
    else {
        return;
    };
    let far = match projection {
        Projection::Perspective(p) => p.far,
        _ => crate::view::PROJECTION_FAR,
    };
    *transform = Transform::from_translation(eye.translation())
        .with_scale(Vec3::splat(far * LAYER_SCALE_OF_FAR));
    *global = GlobalTransform::from(*transform);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_is_the_clients_rings_with_its_rim_on_the_horizon() {
        let mesh = layer_mesh();
        assert_eq!(mesh.count_vertices(), 12 * AZ_STEPS);
        let Some(Indices::U32(idx)) = mesh.indices() else {
            panic!("u32 indices");
        };
        assert_eq!(idx.len(), 11 * AZ_STEPS * 6);
        let pos = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .expect("positions");
        for p in pos {
            let r = Vec3::from(*p).length();
            assert!((r - 1.0).abs() < 1e-5, "radius {r}");
        }
        assert!((pos[0][1] - 1.0).abs() < 1e-6);
        assert!(pos[11 * AZ_STEPS][1].abs() < 1e-6);
    }
}
