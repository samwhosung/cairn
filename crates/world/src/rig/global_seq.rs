use bevy::prelude::*;

use super::AnimParked;
use super::bake::GlobalBone;
use super::compose::PosePost;
use super::pose::RigPose;

/// A rig's free-running bone channels: they loop on the world's clock from the moment the model
/// appeared, whatever sequence plays, and overwrite only the components they drive.
#[derive(Component)]
pub struct GlobalSeqDrive {
    bones: Vec<GlobalBone>,
    appeared_at: Option<f64>,
}

impl GlobalSeqDrive {
    /// `None` when the model has no global-sequence channel on a bone it has.
    pub fn new(global_bones: &[GlobalBone], nbones: usize) -> Option<Self> {
        let bones: Vec<_> = global_bones
            .iter()
            .filter(|g| (g.bone as usize) < nbones)
            .cloned()
            .collect();
        (!bones.is_empty()).then_some(Self {
            bones,
            appeared_at: None,
        })
    }
}

fn apply_global_sequences(
    time: Res<'_, Time>,
    mut drives: Query<'_, '_, (&mut GlobalSeqDrive, &mut RigPose, Has<AnimParked>)>,
) {
    let now = time.elapsed_secs_f64();
    for (mut drive, mut rig, parked) in &mut drives {
        let t = now - *drive.appeared_at.get_or_insert(now);
        if parked {
            continue;
        }
        rig.pose_dirty = true;
        for bone in &drive.bones {
            let Some(tf) = rig.locals.get_mut(bone.bone as usize) else {
                continue;
            };
            let at = |period: f32| (t % f64::from(period.max(1e-3))) as f32;
            if let Some(c) = &bone.translation {
                tf.translation = c.sample(at(c.period));
            }
            if let Some(c) = &bone.rotation {
                tf.rotation = c.sample(at(c.period));
            }
            if let Some(c) = &bone.scale {
                tf.scale = c.sample(at(c.period));
            }
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(PostUpdate, apply_global_sequences.in_set(PosePost));
}
