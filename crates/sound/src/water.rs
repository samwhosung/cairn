use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;
use world::collision::Liquids;
use world::coords::bevy_to_wow;
use world::interior::UnitRoom;

use crate::config::SoundConfig;
use crate::footsteps::{SoundBody, claim_of};
use crate::kit::{KitRef, PlayExtras, SoundCategory, SoundKits, play_kit_ext, source_kit_playing};
use crate::{AudioListener, SoundOutput};

/// `CharacterSplashSoundMedium`, for every body.
const SPLASH_KIT: u32 = 1096;
const SPLASH_DEPTH_FRAC: f32 = 0.4;

type Splashing<'a> = (Entity, &'a Transform, &'a SoundBody, Option<&'a UnitRoom>);
type Moved = Or<(Changed<Transform>, Changed<SoundBody>)>;

pub(crate) fn water_splashes(
    bodies: Query<'_, '_, Splashing<'_>, Moved>,
    liquids: Liquids<'_, '_>,
    mut wet: Local<'_, EntityHashMap<bool>>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
) {
    let Some(mut kits) = kits else {
        return;
    };
    let listener = listener.pos;
    for (entity, transform, body, room) in &bodies {
        let wow = bevy_to_wow(transform.translation);
        let submerged = liquids
            .water_surface_at(wow, claim_of(room))
            .is_some_and(|s| s - wow[2] > SPLASH_DEPTH_FRAC * body.collision_height);
        let was = wet.insert(entity, submerged);
        if was.is_some_and(|w| w != submerged) && !source_kit_playing(&out, entity, SPLASH_KIT) {
            let played = play_kit_ext(
                &mut kits,
                &mut out,
                &config,
                listener,
                KitRef::Id(SPLASH_KIT),
                Some(transform.translation),
                SoundCategory::Sfx,
                PlayExtras {
                    source: Some(entity),
                    ..PlayExtras::default()
                },
            );
            if let Err(e) = played {
                warn!("water splash: {e:#}");
            }
        }
    }
}
