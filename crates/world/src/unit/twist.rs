//! The strafe pose: a body drawn turned off its aim twists its lower spine and its head back
//! toward it.

use std::f32::consts::FRAC_PI_4;

use bevy::prelude::*;

use crate::rig::RigPose;

/// The counter-twist of a unit whose skeleton has a lower spine or a head key bone. Its owner
/// writes the gap; with no gap the bones keep their animated pose.
#[derive(Component)]
pub struct BodyTwist {
    /// The aim's yaw less the drawn body's, radians, wrapped to `(-π, π]`.
    pub yaw_gap: f32,
    spine: Option<Channel>,
    head: Option<Channel>,
}

impl BodyTwist {
    pub(crate) fn new(spine: Option<u16>, head: Option<u16>) -> Self {
        Self {
            yaw_gap: 0.0,
            spine: spine.map(Channel::new),
            head: head.map(Channel::new),
        }
    }
}

struct Channel {
    bone: u16,
    /// The animated rotation the twist was last composed onto.
    base: Quat,
    /// What was last written: a bone still holding it was not keyed by this frame's clip, so its
    /// base is the one from before.
    last_out: Quat,
}

impl Channel {
    fn new(bone: u16) -> Self {
        Self {
            bone,
            base: Quat::IDENTITY,
            last_out: Quat::IDENTITY,
        }
    }
}

/// The spine takes half the gap and the head the rest, each at most 45°: a pure strafe's 90°
/// turns the head exactly back onto the aim.
fn twist_shares(gap: f32) -> (f32, f32) {
    let spine = (gap * 0.5).clamp(-FRAC_PI_4, FRAC_PI_4);
    let head = (gap - spine).clamp(-FRAC_PI_4, FRAC_PI_4);
    (spine, head)
}

/// Each channel yaws its bone's subtree about the model's up axis through the bone's own pivot.
/// The head runs after the spine, so it turns from where the spine left it.
pub(crate) fn apply_body_twist(mut units: Query<'_, '_, (&mut BodyTwist, &mut RigPose)>) {
    for (twist, rig) in &mut units {
        let (twist, rig) = (twist.into_inner(), rig.into_inner());
        let (spine, head) = twist_shares(twist.yaw_gap);
        for (channel, angle) in [(&mut twist.spine, spine), (&mut twist.head, head)] {
            let Some(ch) = channel else { continue };
            let bone = usize::from(ch.bone);
            let Some(cur) = rig.locals.get(bone).map(|t| t.rotation) else {
                continue;
            };
            let base = if cur == ch.last_out { ch.base } else { cur };
            let out = if angle == 0.0 {
                base
            } else {
                let mut model = base;
                let mut up = rig.parents.get(bone).copied().unwrap_or(-1);
                while let Some(p) = usize::try_from(up).ok().filter(|&p| p < rig.locals.len()) {
                    model = rig.locals[p].rotation * model;
                    up = rig.parents[p];
                }
                (base * Quat::from_axis_angle(model.inverse() * Vec3::Y, angle)).normalize()
            };
            ch.base = base;
            ch.last_out = out;
            if out != cur {
                rig.locals[bone].rotation = out;
                rig.pose_dirty = true;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use super::*;
    use crate::rig::{ModelJoint, ModelSkeleton};

    #[test]
    fn a_pure_strafe_closes_on_the_aim_and_each_share_caps_at_45_degrees() {
        assert_eq!(twist_shares(-FRAC_PI_2), (-FRAC_PI_4, -FRAC_PI_4));
        assert_eq!(twist_shares(FRAC_PI_4), (FRAC_PI_4 / 2.0, FRAC_PI_4 / 2.0));
        assert_eq!(twist_shares(PI), (FRAC_PI_4, FRAC_PI_4));
        assert_eq!(twist_shares(0.0), (0.0, 0.0));
    }

    fn joint(parent: i16) -> ModelJoint {
        ModelJoint {
            parent,
            local_translation: Vec3::Y,
            billboard: None,
            parent_arm: None,
        }
    }

    /// A root, a lower spine leaning forward, and a head: the head's frame ends up yawed the
    /// whole gap in the model, however the spine leans.
    #[test]
    fn the_head_lands_on_the_aim_through_a_leaning_spine() {
        let skeleton = ModelSkeleton {
            joints: vec![joint(-1), joint(0), joint(1)],
            spine_bone: Some(1),
            head_bone: Some(2),
        };
        let mut app = App::new();
        app.add_systems(Update, apply_body_twist);
        let unit = app.world_mut().spawn_empty().id();
        let mut rig = RigPose::new(unit, &skeleton);
        let lean = Quat::from_rotation_x(0.4);
        rig.locals[1].rotation = lean;
        let mut twist = BodyTwist::new(Some(1), Some(2));
        twist.yaw_gap = FRAC_PI_2;
        app.world_mut().entity_mut(unit).insert((rig, twist));
        app.update();
        let rig = app.world().get::<RigPose>(unit).expect("the rig");
        let head = rig.locals[0].rotation * rig.locals[1].rotation * rig.locals[2].rotation;
        let want = Quat::from_rotation_y(FRAC_PI_2) * lean;
        assert!(head.angle_between(want) < 1e-5, "{head} against {want}");
        let spine = rig.locals[0].rotation * rig.locals[1].rotation;
        let want = Quat::from_rotation_y(FRAC_PI_4) * lean;
        assert!(spine.angle_between(want) < 1e-5, "{spine} against {want}");
    }

    #[test]
    fn a_closed_gap_hands_the_bones_back_to_the_animation() {
        let skeleton = ModelSkeleton {
            joints: vec![joint(-1), joint(0)],
            spine_bone: Some(1),
            head_bone: None,
        };
        let mut app = App::new();
        app.add_systems(Update, apply_body_twist);
        let unit = app.world_mut().spawn_empty().id();
        let mut twist = BodyTwist::new(Some(1), None);
        twist.yaw_gap = 1.0;
        app.world_mut()
            .entity_mut(unit)
            .insert((RigPose::new(unit, &skeleton), twist));
        app.update();
        app.update();
        app.world_mut()
            .get_mut::<BodyTwist>(unit)
            .expect("the twist")
            .yaw_gap = 0.0;
        app.update();
        let rig = app.world().get::<RigPose>(unit).expect("the rig");
        assert_eq!(rig.locals[1].rotation, Quat::IDENTITY);
    }
}
