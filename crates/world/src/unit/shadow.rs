//! The blob shadow: a soft dark oval under every unit, its model's Stand box projected onto the
//! ground and multiplied into it, as faint as the unit is.

use bevy::prelude::*;

use super::body::UnitBody;
use super::fade::{UnitAlpha, UnitAppear};
use crate::decal::{DecalFrame, WorldDecal};
use crate::effects::{EffectVertex, WorldEffectDraw};
use crate::rig::ModelAnimations;
use crate::sky_order::{DECAL_RASTER, SHADOW_SORT_RUNG};
use crate::source::{Repeat, texture_url};
use crate::view::WorldCamera;

const TEXTURE: &str = "Textures\\ShadowBlob.blp";
/// Each coordinate of the Stand box is clamped to ±this before the unit's scale.
const BOX_CLAMP: f32 = 5.0;
/// The client's width under which a box casts nothing.
const DEGENERATE: f32 = 2.384e-7;
/// The client's box reaches this many times farther below the feet than above them.
const DOWN_PER_UP: f32 = 5.0 / 3.0;

#[derive(Resource)]
pub(crate) struct ShadowTexture(Handle<Image>);

pub(crate) fn load_texture(mut commands: Commands<'_, '_>, server: Res<'_, AssetServer>) {
    commands.insert_resource(ShadowTexture(
        server.load(texture_url(TEXTURE, Repeat::BOTH)),
    ));
}

/// A unit's shadow as last projected; a shown one is projected again only when what it was
/// projected from changes.
#[derive(Component, Default)]
pub(crate) struct BlobShadow {
    shown_from: Option<ShadowKey>,
    verts: Vec<EffectVertex>,
}

#[derive(Clone, Copy, PartialEq)]
struct ShadowKey {
    feet: Vec3,
    rotation: Quat,
    box_min: Vec3,
    box_max: Vec3,
    alpha: f32,
    receivers: usize,
}

impl ShadowKey {
    fn differs_from(&self, was: &Self) -> bool {
        const YARDS: f32 = 1e-3;
        self.feet.distance_squared(was.feet) > YARDS * YARDS
            || self.rotation.angle_between(was.rotation) > 1e-3
            || (self.box_min - was.box_min).abs().max_element() > YARDS
            || (self.box_max - was.box_max).abs().max_element() > YARDS
            || (self.alpha - was.alpha).abs() > 1.0 / 255.0
            || self.receivers != was.receivers
    }
}

/// The client's alpha across the box's height: up over its bottom sixth, down over its top
/// sixth.
fn height_ramp(u: f32) -> f32 {
    let x = 12.0 * u.clamp(0.0, 1.0);
    if x < 2.0 {
        0.5 * x
    } else if x < 10.0 {
        1.0
    } else {
        (0.5 * (12.0 - x)).max(0.0)
    }
}

fn shadow_frame(feet: Vec3, rotation: Quat, scale: f32, stand: (Vec3, Vec3)) -> Option<DecalFrame> {
    let clamp = |v: Vec3| v.clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP)) * scale.max(0.0);
    let (bmin, bmax) = (clamp(stand.0), clamp(stand.1));
    if bmax.x - bmin.x <= DEGENERATE || bmax.z - bmin.z <= DEGENERATE {
        return None;
    }
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (x, z) in [
        (bmin.x, bmin.z),
        (bmin.x, bmax.z),
        (bmax.x, bmin.z),
        (bmax.x, bmax.z),
    ] {
        let w = rotation * Vec3::new(x, 0.0, z);
        lo = lo.min(Vec2::new(w.x, w.z));
        hi = hi.max(Vec2::new(w.x, w.z));
    }
    let half_height = (bmax.y - bmin.y) * 0.5;
    Some(DecalFrame {
        center: feet,
        turn: Rot2::IDENTITY,
        min_x: lo.x,
        max_x: hi.x,
        min_z: lo.y,
        max_z: hi.y,
        min_y: -half_height * DOWN_PER_UP,
        max_y: half_height,
    })
}

type Casters<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static ModelAnimations,
        Option<&'static UnitAppear>,
        Option<&'static UnitAlpha>,
        Option<&'static InheritedVisibility>,
        &'static mut BlobShadow,
    ),
    With<UnitBody>,
>;

pub(crate) fn update_shadows(
    time: Res<'_, Time>,
    decals: WorldDecal<'_, '_>,
    mut casters: Casters<'_, '_>,
) {
    let now = time.elapsed_secs();
    let receivers = decals.receiver_count();
    for (unit, anims, appear, owner, drawn, mut shadow) in &mut casters {
        let alpha = match appear {
            Some(appear) => appear.alpha(now),
            None => owner.map_or(1.0, |o| o.alpha),
        }
        .clamp(0.0, 1.0);
        // The client sizes it by the Stand's box, whatever the unit is playing.
        let stand = anims
            .find(anims.resolve(0, &|_| None).id)
            .map(|c| (c.bounds_min, c.bounds_max));
        let frame = stand
            .filter(|_| alpha > 0.0 && drawn.is_none_or(|v| v.get()))
            .and_then(|b| shadow_frame(unit.translation, unit.rotation, unit.scale.x, b));
        let (Some(frame), Some((box_min, box_max))) = (frame, stand) else {
            if shadow.shown_from.is_some() {
                shadow.shown_from = None;
                shadow.verts.clear();
            }
            continue;
        };
        let key = ShadowKey {
            feet: unit.translation,
            rotation: unit.rotation,
            box_min,
            box_max,
            alpha,
            receivers,
        };
        if shadow
            .shown_from
            .as_ref()
            .is_some_and(|was| !key.differs_from(was))
        {
            continue;
        }
        let shadow = shadow.into_inner();
        shadow.verts.clear();
        let span = frame.max_y - frame.min_y;
        let shown = decals.project(
            &mut shadow.verts,
            &frame,
            |p| alpha * height_ramp((p.y - frame.min_y) / span),
            |x, z| frame.rect_uv(x, z),
        );
        shadow.shown_from = shown.then_some(key);
    }
}

pub(crate) fn push_shadows(
    texture: Option<Res<'_, ShadowTexture>>,
    camera: Query<'_, '_, Entity, With<WorldCamera>>,
    mut draw: WorldEffectDraw<'_>,
    casters: Query<'_, '_, (Entity, &BlobShadow)>,
) {
    let (Some(texture), Ok(cam)) = (texture, camera.single()) else {
        return;
    };
    for (unit, shadow) in &casters {
        let Some(key) = shadow.shown_from else {
            continue;
        };
        let mut batch = draw
            .batch(cam, texture.0.id())
            .multiply()
            .anchored(key.feet)
            .rung(SHADOW_SORT_RUNG, DECAL_RASTER)
            .owner(unit);
        batch.vertices(&shadow.verts);
        batch.tris();
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests;
