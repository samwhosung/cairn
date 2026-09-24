//! A footfall's sound: the `$FSD` key through the surface under the foot to the body's footstep
//! class and a kit. Wading picks the splash kit where the class has one; deeper than the wade the
//! body swims and its steps are silent. A body without a footstep class is silent-footed.

use bevy::prelude::*;
use world::collision::{LiquidClaim, Liquids};
use world::coords::bevy_to_wow;
use world::interior::UnitRoom;
use world::rig_events::{AnimEvent, is_footstep_sound};
use world::surface::{SurfaceUnderfoot, Underfoot};

use crate::config::SoundConfig;
use crate::kit::{Bus, KitRef, PlayExtras, SoundCategory, SoundKits, play_kit_ext};
use crate::tables::{CreatureVoices, Footsteps};
use crate::{AudioListener, SoundOutput};

/// A body that makes sounds: its creature display, its collision height, and how deep it wades
/// before it swims.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct SoundBody {
    pub display: u32,
    pub collision_height: f32,
    pub wade_max: f32,
}

/// The claim a body's liquid is scoped by: its room's building, or the open world.
pub(crate) fn claim_of(room: Option<&UnitRoom>) -> LiquidClaim {
    match room.map(UnitRoom::room) {
        Some(Some(_)) => LiquidClaim::Inside,
        Some(None) => LiquidClaim::Outdoors,
        None => LiquidClaim::Unknown,
    }
}

pub(crate) fn load_footsteps(
    mut commands: Commands<'_, '_>,
    install: Option<Res<'_, world::Install>>,
) {
    let Some(install) = install else { return };
    match Footsteps::load(&install.0) {
        Ok(cat) => commands.insert_resource(cat),
        Err(e) => warn!("sound: no footsteps: {e}"),
    }
    match CreatureVoices::load(&install.0) {
        Ok(cat) => commands.insert_resource(cat),
        Err(e) => warn!("sound: no creature voices, so no footsteps: {e}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn footstep_sounds(
    mut events: MessageReader<'_, '_, AnimEvent>,
    bodies: Query<'_, '_, (&SoundBody, &GlobalTransform, Option<&UnitRoom>)>,
    footsteps: Option<Res<'_, Footsteps>>,
    voices: Option<Res<'_, CreatureVoices>>,
    surface: SurfaceUnderfoot<'_, '_>,
    liquids: Liquids<'_, '_>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(footsteps), Some(voices), Some(mut kits)) = (footsteps, voices, kits) else {
        events.clear();
        return;
    };
    let listener = listener.pos;
    for ev in events.read() {
        if !is_footstep_sound(&ev.ident) {
            continue;
        }
        let Ok((body, transform, room)) = bodies.get(ev.entity) else {
            continue;
        };
        let Some(class) = voices.footstep_class(body.display).filter(|&c| c != 0) else {
            continue;
        };
        let feet = transform.translation();
        let wow = bevy_to_wow(feet);
        let depth = liquids
            .water_surface_at(wow, claim_of(room))
            .map(|s| s - wow[2])
            .filter(|d| *d > 0.0);
        if depth.is_some_and(|d| d > body.wade_max) {
            continue;
        }
        let terrain = match surface.at(room.and_then(UnitRoom::room), feet) {
            Some(Underfoot::Terrain(t)) => Some(t),
            Some(Underfoot::GroundEffect(e)) => footsteps.terrain_of(e),
            None => None,
        };
        let Some(terrain) = terrain else { continue };
        let Some((dry, splash)) = footsteps.resolve_terrain(class, terrain) else {
            continue;
        };
        let kit = match depth {
            Some(_) if splash != 0 => splash,
            _ => dry,
        };
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit_ext(
            &mut kits,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(ev.pos.unwrap_or(feet)),
            SoundCategory::Sfx,
            PlayExtras {
                bus: Bus::FOOTSTEP,
                ..PlayExtras::default()
            },
        ) {
            warn!("footstep (kit {kit}): {e:#}");
        }
    }
}
