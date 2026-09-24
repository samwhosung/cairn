//! M2 ribbon trails, the streamers behind a moving bone.

use std::collections::VecDeque;

use bevy::camera::Projection;
use bevy::camera::primitives::Frustum;
use bevy::prelude::*;
use model::RibbonEmitterDef;

use crate::coords::wow_to_bevy;
use crate::effects::{EffectDrawSpec, EffectFog, EffectLighting, EffectQuads, EffectVertex};
use crate::m2::ModelRibbon;
use crate::particles::{DrawSetGate, MAX_STEP, SceneGates, playing_seq};
use crate::rig::ModelAnimations;
use crate::unit::UnitAlpha;
use crate::view::{WorldCamera, nearest_depth_within_farclip};

pub(crate) const MAX_EDGES: usize = 512;

struct Edge {
    top: Vec3,
    bottom: Vec3,
    born: f32,
    age: f32,
}

fn gravity_step(gravity: f32, age: f32, dt: f32) -> f32 {
    gravity * ((age + dt).powi(2) - age.powi(2))
}

fn hold_edge_ages(edges: &mut VecDeque<Edge>, dt: f32) {
    for e in edges {
        e.born += dt;
    }
}

#[derive(Component)]
pub struct RibbonTrail {
    def: RibbonEmitterDef,
    offset_in_owner: Vec3,
    owner: Option<Entity>,
    seq: RibbonSeq,
    alpha_src: Option<Entity>,
    fade: Option<DrawSetGate>,
    edges_oldest_first: VecDeque<Edge>,
    accumulator: f32,
    track_clock: f32,
    texture: Handle<Image>,
    bias: f32,
}

impl RibbonTrail {
    pub fn edge_count(&self) -> usize {
        self.edges_oldest_first.len()
    }
}

/// What a trail's on/off track reads.
#[derive(Clone, Copy)]
pub enum RibbonSeq {
    /// The animation this entity's player plays.
    Host(Entity),
    /// One animation's opening value, for good.
    Fixed(u16),
}

/// Spawns a trail riding `owner`; with `use_pivot` the owner is the emitter bone's joint and the
/// record's position is rebased by the bone's pivot. `owner_scale` takes the model's reach to
/// world yards. `None` without a texture or with nothing to trail.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_ribbon(
    commands: &mut Commands<'_, '_>,
    ribbon: &ModelRibbon,
    owner: Entity,
    use_pivot: bool,
    owner_scale: f32,
    seq: RibbonSeq,
    alpha_src: Option<Entity>,
    fade: Option<DrawSetGate>,
) -> Option<Entity> {
    let texture = ribbon.texture.clone()?;
    let def = ribbon.def.clone();
    if def.edges_per_second <= 0.0
        || (def.height_above.peak().max(0.0) + def.height_below.peak().max(0.0)) <= 0.0
    {
        return None;
    }
    let p = def.position;
    let local = if use_pivot {
        [
            p[0] - ribbon.bone_pivot[0],
            p[1] - ribbon.bone_pivot[1],
            p[2] - ribbon.bone_pivot[2],
        ]
    } else {
        p
    };
    Some(
        commands
            .spawn((
                Transform::IDENTITY,
                RibbonTrail {
                    offset_in_owner: wow_to_bevy(local),
                    def,
                    owner: Some(owner),
                    seq,
                    alpha_src,
                    fade,
                    edges_oldest_first: VecDeque::new(),
                    accumulator: 0.0,
                    track_clock: 0.0,
                    texture,
                    bias: model::owner_last_rung(ribbon.owner_reach * owner_scale),
                },
            ))
            .id(),
    )
}

type Trails<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut RibbonTrail,
        &'static mut Transform,
        &'static mut GlobalTransform,
    ),
    Without<WorldCamera>,
>;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn simulate_ribbons(
    time: Res<'_, Time>,
    mut commands: Commands<'_, '_>,
    transforms: Query<'_, '_, &GlobalTransform, Without<RibbonTrail>>,
    hosts: Query<'_, '_, (&AnimationPlayer, &ModelAnimations)>,
    images: Res<'_, Assets<Image>>,
    mut quads: ResMut<'_, EffectQuads>,
    alphas: Query<'_, '_, &UnitAlpha>,
    world_cam: Query<'_, '_, (Entity, &GlobalTransform, &Frustum, &Projection), With<WorldCamera>>,
    gates: SceneGates<'_, '_>,
    mut trails: Trails<'_, '_>,
) {
    let Ok((cam, cam_tf, frustum, projection)) = world_cam.single() else {
        return;
    };
    let view = gates.view(cam_tf, projection, frustum);
    let cam_pos = cam_tf.translation();
    let cam_fwd = Vec3::from(cam_tf.forward());
    let dt = time.delta_secs().min(MAX_STEP);
    let now = time.elapsed_secs();
    for (entity, mut trail, mut entity_tf, mut entity_global) in &mut trails {
        let RibbonTrail {
            def,
            offset_in_owner,
            owner,
            seq,
            alpha_src,
            fade,
            edges_oldest_first: edges,
            accumulator,
            track_clock,
            texture,
            bias,
        } = &mut *trail;
        if owner.is_some_and(|o| !transforms.contains(o)) {
            *owner = None;
        }
        let head = owner.and_then(|o| transforms.get(o).ok()).map(|owner_gt| {
            let node = owner_gt.transform_point(*offset_in_owner);
            let cross_axis =
                (owner_gt.rotation() * wow_to_bevy([0.0, 1.0, 0.0])).normalize_or(Vec3::Y);
            (node, cross_axis)
        });
        if head.is_none() && edges.is_empty() {
            commands.entity(entity).despawn();
            continue;
        }
        let admitted = match (head, fade.as_ref()) {
            (Some(_), Some(f)) => f.admitted(&gates, &view),
            (Some((node, _)), None) => nearest_depth_within_farclip(cam_pos, cam_fwd, node, 0.0),
            (None, _) => true,
        };
        if !admitted {
            hold_edge_ages(edges, dt);
            continue;
        }
        *track_clock += dt;
        let ms = *track_clock * 1000.0;
        let h_above = def.height_above.sample_ms(ms).max(0.0);
        let h_below = def.height_below.sample_ms(ms).max(0.0);
        let shown = def.visible.as_ref().is_none_or(|vis| match *seq {
            RibbonSeq::Fixed(a) => vis.at(a, 0.0),
            RibbonSeq::Host(h) => match hosts.get(h) {
                Ok((player, anims)) => {
                    let (anim, t) = playing_seq(player, anims)
                        .and_then(|p| {
                            let clip = anims.clips.iter().find(|c| c.seq_index == p.slot)?;
                            Some((clip.anim_id, p.seconds))
                        })
                        .unwrap_or((0, 0.0));
                    vis.at(anim, t)
                }
                Err(_) => false,
            },
        });
        let head = shown.then_some(head).flatten();
        while edges
            .front()
            .is_some_and(|e| now - e.born >= def.edge_lifetime)
        {
            edges.pop_front();
        }
        for e in edges.iter_mut() {
            let term = gravity_step(def.gravity, e.age, dt);
            e.top.y += term;
            e.bottom.y += term;
            e.age += dt;
        }
        if let Some((node, axis)) = head {
            *accumulator += def.edges_per_second * dt;
            let n = accumulator.floor().max(0.0);
            *accumulator -= n;
            let n = (n as usize).min(MAX_EDGES.saturating_sub(edges.len()));
            for k in 0..n {
                let back = dt * (n - 1 - k) as f32 / n as f32;
                edges.push_back(Edge {
                    top: node + axis * h_above,
                    bottom: node - axis * h_below,
                    born: now - back,
                    age: back,
                });
            }
        }
        if !images.contains(&*texture) || !shown {
            continue;
        }
        if alpha_src.is_some_and(|e| alphas.get(e).is_ok_and(|a| a.alpha <= 1e-3)) {
            continue;
        }
        let n = edges.len() + usize::from(head.is_some());
        if n < 2 {
            continue;
        }
        let (rows, cols) = (def.tile_rows.max(1), def.tile_cols.max(1));
        let cell = def.tex_slot.min(rows * cols - 1);
        let (u0, u1) = (
            f32::from(cell % cols) / f32::from(cols),
            f32::from(cell % cols + 1) / f32::from(cols),
        );
        let (v0, v1) = (
            f32::from(cell / cols) / f32::from(rows),
            f32::from(cell / cols + 1) / f32::from(rows),
        );
        let rgb = def.color.sample_ms(ms);
        let rgba = [rgb[0], rgb[1], rgb[2], def.alpha.sample_ms(ms).max(0.0)];
        let anchor = head.map_or_else(
            || {
                edges
                    .back()
                    .map_or(Vec3::ZERO, |e| (e.top + e.bottom) * 0.5)
            },
            |(node, _)| node,
        );
        entity_tf.translation = anchor;
        *entity_global = GlobalTransform::from(*entity_tf);
        let mut pairs: Vec<(Vec3, Vec3, f32)> = Vec::with_capacity(n);
        if let Some((node, axis)) = head {
            pairs.push((node + axis * h_above, node - axis * h_below, 0.0));
        }
        for e in edges.iter().rev() {
            pairs.push((
                e.top,
                e.bottom,
                ((now - e.born) / def.edge_lifetime).clamp(0.0, 1.0),
            ));
        }
        let start = quads.begin();
        for w in pairs.windows(2) {
            let ((t0, b0, a0), (t1, b1, a1)) = (w[0], w[1]);
            let (ua0, ua1) = (u0 + (u1 - u0) * a0, u0 + (u1 - u0) * a1);
            for (pos, uv) in [
                (b0, [ua0, v1]),
                (b1, [ua1, v1]),
                (t1, [ua1, v0]),
                (t0, [ua0, v0]),
            ] {
                quads.verts.push(EffectVertex {
                    pos: pos.to_array(),
                    uv,
                    color: rgba,
                });
            }
        }
        quads.commit_quads(
            start,
            EffectDrawSpec {
                cam,
                texture: texture.id(),
                blend: def.blend.into(),
                fog: EffectFog::for_blend(0, def.blend),
                lighting: EffectLighting::None,
                sort_anchor: anchor,
                sort_bias: *bias,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: entity,
            },
        );
    }
}

pub(crate) struct RibbonPlugin;

impl Plugin for RibbonPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            simulate_ribbons
                .after(crate::effects::begin_effect_frame)
                .after(crate::rig::RigFinalize)
                .after(crate::billboard::face_billboards),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::gravity_step;

    #[test]
    fn gravity_sums_to_g_t_squared_at_any_frame_rate() {
        for &g in &[0.5_f32, 1.0, 2.0, 5.0, -1.0] {
            for &dt in &[1.0 / 144.0_f32, 1.0 / 60.0, 1.0 / 30.0, 1.0 / 15.0] {
                let steps = (1.0 / dt).round() as usize;
                let (mut z, mut age) = (0.0_f32, 0.0_f32);
                for _ in 0..steps {
                    z += gravity_step(g, age, dt);
                    age += dt;
                }
                let t = steps as f32 * dt;
                assert!((z - g * t * t).abs() < 1e-4);
            }
        }
        assert!(
            gravity_step(2.0, 0.0, 0.016) > 0.0,
            "a positive gravity rises"
        );
    }
}
