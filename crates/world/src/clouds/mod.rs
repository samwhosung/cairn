mod kernel;
mod layer;
mod tables;

use bevy::asset::embedded_asset;
use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::light::SceneLight;
use crate::view::WorldCamera;

pub use layer::CloudMaterial;

/// How the clouds keep time. Live, a band of the field regenerates every tenth of a second and
/// the clouds slowly change shape. Held, the field is built once for the sky it is in and again
/// only when that sky's cloud density changes, so a shot's clouds are the same on every run.
#[derive(Resource, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum CloudClock {
    #[default]
    Live,
    Held,
}

#[derive(Resource, Default)]
pub(crate) struct CloudCoverage {
    kernel: kernel::CloudKernel,
    primed: bool,
    density: f32,
    frame: Option<kernel::CloudFrame>,
}

pub(crate) struct CloudsPlugin;

impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "cloud.wgsl");
        app.add_plugins(MaterialPlugin::<CloudMaterial>::default())
            .init_resource::<CloudCoverage>()
            .init_resource::<CloudClock>()
            .add_systems(Startup, layer::spawn_layer)
            .add_systems(Update, tick_clouds.after(crate::atmosphere::resolve_light))
            .add_systems(
                PostUpdate,
                layer::follow_camera.after(TransformSystems::Propagate),
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn tick_clouds(
    mut coverage: ResMut<'_, CloudCoverage>,
    clock: Res<'_, CloudClock>,
    light: Res<'_, SceneLight>,
    time: Res<'_, Time>,
    camera: Query<'_, '_, (), With<WorldCamera>>,
    layer: Option<Res<'_, layer::CloudImage>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut materials: ResMut<'_, Assets<CloudMaterial>>,
) {
    if camera.is_empty() {
        return;
    }
    let [glow, slope, base] = light.cloud_colors;
    let frame = kernel::CloudFrame {
        glow,
        slope,
        base,
        glow_dir: light.cloud_glow_dir,
        glow_track: light.cloud_glow,
    };
    let density = light.cloud_density;
    let held = *clock == CloudClock::Held;
    let cov = &mut *coverage;
    let changed = if !cov.primed || (held && density.to_bits() != cov.density.to_bits()) {
        cov.primed = true;
        cov.density = density;
        cov.frame = Some(frame);
        cov.kernel.rebuild(density, &frame);
        true
    } else if held {
        let recolor = cov.frame != Some(frame);
        if recolor {
            cov.frame = Some(frame);
            cov.kernel.recolor(&frame);
        }
        recolor
    } else {
        cov.kernel.regen_if_due(time.delta_secs(), density, &frame)
    };
    let Some(layer) = layer.filter(|_| changed) else {
        return;
    };
    if let Some(data) = images.get_mut(&layer.image).and_then(|i| i.data.as_mut()) {
        data.copy_from_slice(cov.kernel.rgba().as_flattened());
        // A re-uploaded image is a new texture, which the material only binds once it is touched.
        materials.get_mut(&layer.material);
    }
}
