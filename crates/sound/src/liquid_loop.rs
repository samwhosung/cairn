//! The liquid beds layered over the zone's. At most [`MAX_CONCURRENT`] liquid classes are armed,
//! water over ocean over magma over slime.

use bevy::prelude::*;
use world::collision::Liquids;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::submersion::Underwater;

use crate::config::SoundConfig;
use crate::footsteps::SoundBody;
use crate::kit::{
    KitRef, PlayExtras, SoundCategory, SoundKits, play_kit_ext, set_source_kit_gain,
    source_kit_playing, stop_source_kit,
};
use crate::tables::WaterSounds;
use crate::{AudioListener, SoundOutput};

/// How near a liquid of a class arms that class's loop, yards.
pub const LIQUID_LOOP_REACH: f32 = 9.0;
const SLEW_PER_TICK: f32 = 0.166_67;
const LIQUID_CELL_DIAGONAL: f32 = 5.892_557;
const FADE_SECS: f32 = 5.0;
const MAX_CONCURRENT: usize = 2;

struct ClassLoop {
    emitter: Entity,
    kit: u32,
    gain: f32,
    retiring: Option<(u32, f32)>,
}

#[derive(Resource, Default)]
pub(crate) struct LiquidLoopState {
    classes: [Option<ClassLoop>; 4],
    was_underwater: bool,
}

pub(crate) fn load_water_sounds(
    mut commands: Commands<'_, '_>,
    install: Option<Res<'_, world::Install>>,
) {
    let Some(install) = install else { return };
    match WaterSounds::load(&install.0) {
        Ok(cat) => commands.insert_resource(cat),
        Err(e) => warn!("sound: no liquid loops: {e}"),
    }
}

/// The player's body: the one with a sound body the listener sits on.
#[derive(Component)]
pub struct Listening;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn drive_liquid_loops(
    mut state: ResMut<'_, LiquidLoopState>,
    water_sounds: Option<Res<'_, WaterSounds>>,
    liquids: Liquids<'_, '_>,
    underwater: Res<'_, Underwater>,
    player: Query<'_, '_, &Transform, (With<SoundBody>, With<Listening>)>,
    mut emitters: Query<'_, '_, &mut Transform, Without<SoundBody>>,
    time: Res<'_, Time>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
    mut commands: Commands<'_, '_>,
) {
    let (Some(water_sounds), Some(mut kits)) = (water_sounds, kits) else {
        return;
    };
    if out.mixer.is_none() {
        return;
    }
    if underwater.0.is_water() {
        for slot in &mut state.classes {
            if let Some(cl) = slot.take() {
                stop_source_kit(&mut out, cl.emitter, cl.kit);
                if let Some((old, _)) = cl.retiring {
                    stop_source_kit(&mut out, cl.emitter, old);
                }
            }
        }
        state.was_underwater = true;
        return;
    }
    let resurfaced = std::mem::take(&mut state.was_underwater);
    let Ok(player) = player.single() else {
        return;
    };
    let player_pos = player.translation;
    let best = liquids.nearest_per_class(bevy_to_wow(player_pos), LIQUID_LOOP_REACH);
    let mut budget = MAX_CONCURRENT;
    let fade_step = time.delta_secs() / FADE_SECS;
    let start = |kits: &mut SoundKits, out: &mut SoundOutput, kit, pos, emitter, gain| {
        let played = play_kit_ext(
            kits,
            out,
            &config,
            listener.pos,
            KitRef::Id(kit),
            Some(pos),
            SoundCategory::Ambience,
            PlayExtras {
                source: Some(emitter),
                force_loop: true,
                ..PlayExtras::default()
            },
        );
        match played {
            Ok(_) => set_source_kit_gain(out, emitter, kit, gain),
            Err(e) => warn!("liquid loop kit {kit}: {e:#}"),
        }
    };
    for (class, scanned) in best.into_iter().enumerate() {
        let candidate = scanned.filter(|_| budget > 0);
        let desired = candidate.and_then(|c| water_sounds.kit_for_nibble(c.nibble));
        if desired.is_some() {
            budget -= 1;
        }
        let slot = &mut state.classes[class];
        match (slot.as_mut(), desired, candidate) {
            (None, Some(kit), Some(c)) => {
                let pos = near_clamped(wow_to_bevy(c.point), player_pos);
                let emitter = commands.spawn(Transform::from_translation(pos)).id();
                let gain = if resurfaced { 1.0 } else { 0.0 };
                start(&mut kits, &mut out, kit, pos, emitter, gain);
                *slot = Some(ClassLoop {
                    emitter,
                    kit,
                    gain,
                    retiring: None,
                });
            }
            (Some(cl), Some(kit), Some(c)) => {
                if cl.kit != kit {
                    if let Some((old, _)) = cl.retiring.take() {
                        stop_source_kit(&mut out, cl.emitter, old);
                    }
                    cl.retiring = Some((cl.kit, cl.gain));
                    cl.kit = kit;
                    cl.gain = 0.0;
                }
                if let Ok(mut tf) = emitters.get_mut(cl.emitter) {
                    let target = near_clamped(wow_to_bevy(c.point), player_pos);
                    let step = target - tf.translation;
                    let len = step.length();
                    tf.translation += if len > SLEW_PER_TICK {
                        step * (SLEW_PER_TICK / len)
                    } else {
                        step
                    };
                }
                cl.gain = if resurfaced {
                    1.0
                } else {
                    (cl.gain + fade_step).min(1.0)
                };
                if !source_kit_playing(&out, cl.emitter, cl.kit) {
                    let pos = emitters
                        .get(cl.emitter)
                        .map_or(player_pos, |t| t.translation);
                    start(&mut kits, &mut out, cl.kit, pos, cl.emitter, cl.gain);
                }
                set_source_kit_gain(&mut out, cl.emitter, cl.kit, cl.gain);
            }
            (Some(cl), None, _) => {
                cl.gain -= fade_step;
                if cl.gain <= 0.0 {
                    stop_source_kit(&mut out, cl.emitter, cl.kit);
                    if let Some((old, _)) = cl.retiring.take() {
                        stop_source_kit(&mut out, cl.emitter, old);
                    }
                    let emitter = cl.emitter;
                    *slot = None;
                    commands.entity(emitter).despawn();
                } else {
                    set_source_kit_gain(&mut out, cl.emitter, cl.kit, cl.gain);
                }
            }
            _ => {}
        }
        if let Some(cl) = state.classes[class].as_mut()
            && let Some((old, mut g)) = cl.retiring.take()
        {
            g -= fade_step;
            if g <= 0.0 {
                stop_source_kit(&mut out, cl.emitter, old);
            } else {
                set_source_kit_gain(&mut out, cl.emitter, old, g);
                cl.retiring = Some((old, g));
            }
        }
    }
}

fn near_clamped(pos: Vec3, player: Vec3) -> Vec3 {
    let d = pos - player;
    let len = d.length();
    if len >= LIQUID_CELL_DIAGONAL {
        pos
    } else if len > 1e-4 {
        player + d * (LIQUID_CELL_DIAGONAL / len)
    } else {
        player + Vec3::X * LIQUID_CELL_DIAGONAL
    }
}
