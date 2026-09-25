mod kernel;
mod layer;
mod tables;

use bevy::asset::embedded_asset;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::celestial::SPRITE_SPHERE_YARDS;
use crate::draw_order::OrderedMaterialPlugin;
use crate::light::SceneLight;
use crate::submersion::Underwater;
use crate::view::WorldCamera;

pub(crate) use kernel::{moon_halo, sun_clearance};
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

impl CloudCoverage {
    pub(crate) fn coverage_toward(&self, dir: Vec3) -> f32 {
        self.kernel.coverage(dir * SPRITE_SPHERE_YARDS)
    }
}

pub(crate) struct CloudsPlugin;

impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "cloud.wgsl");
        app.add_plugins(OrderedMaterialPlugin::<CloudMaterial>::default())
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
    underwater: Res<'_, Underwater>,
    mut was_submerged: Local<'_, bool>,
    mut images: ResMut<'_, Assets<Image>>,
    mut materials: ResMut<'_, Assets<CloudMaterial>>,
) {
    if camera.is_empty() {
        return;
    }
    // The client rebuilds the whole field the frame the eye leaves a liquid, rather than letting
    // its band-by-band regeneration bring the clouds back.
    let submerged = underwater.0.any();
    let surfaced = *was_submerged && !submerged;
    *was_submerged = submerged;
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
    let changed = if !cov.primed || surfaced || (held && density.to_bits() != cov.density.to_bits())
    {
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
