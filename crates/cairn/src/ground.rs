use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use world::coords::wow_to_bevy;

use crate::shot::ShotWaitsFor;
use crate::view::HUMAN_START;

const GRASS: &str = "Tileset\\Elwynn\\ElwynnGrassBase.blp";
const HALF_EXTENT: f32 = 3000.0;

pub struct GrassFieldPlugin;

impl Plugin for GrassFieldPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ground.wgsl");
        app.add_plugins(MaterialPlugin::<GroundMaterial>::default())
            .insert_resource(ClearColor(Color::srgb_u8(118, 166, 222)))
            .add_systems(Startup, spawn);
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct GroundMaterial {
    #[texture(0)]
    #[sampler(1)]
    grass: Handle<Image>,
}

impl Material for GroundMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cairn/ground.wgsl".into()
    }
}

fn spawn(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<GroundMaterial>>,
    mut waits_for: ResMut<'_, ShotWaitsFor>,
) {
    let grass = server.load(world::texture_url(GRASS, world::Repeat::BOTH));
    waits_for.0.push(grass.clone().untyped());
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(HALF_EXTENT)))),
        MeshMaterial3d(materials.add(GroundMaterial { grass })),
        Transform::from_translation(wow_to_bevy(HUMAN_START.to_array())),
    ));
}
