//! The sound of the building room the player is in: its `WMOAreaTable` row's music, fanfare,
//! ambience and reverb, each overriding the area's where it is set.

use bevy::prelude::*;
use world::WmoAreas;
use world::interior::CurrentWmoInterior;

/// The room's audio columns; `None` outdoors or in a room with no `WMOAreaTable` row; zeros where
/// the row sets none.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentInterior(pub Option<InteriorAudio>);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteriorAudio {
    /// `SoundProviderPreferences` ids, `[dry, underwater]`.
    pub sound_provider: [u32; 2],
    pub ambience: u32,
    pub zone_music: u32,
    pub intro_sound: u32,
}

pub(crate) fn resolve_interior(
    interior: Res<'_, CurrentWmoInterior>,
    areas: Option<Res<'_, WmoAreas>>,
    mut current: ResMut<'_, CurrentInterior>,
) {
    let row = interior
        .0
        .zip(areas.as_deref())
        .and_then(|(k, areas)| areas.resolve(k.wmo_id, k.name_set, k.group_area_id));
    let audio = row.as_ref().map(|r| InteriorAudio {
        sound_provider: r.sound_provider,
        ambience: r.ambience,
        zone_music: r.zone_music,
        intro_sound: r.intro_sound,
    });
    if current.0 != audio {
        match &row {
            Some(r) if !r.name.is_empty() => info!("interior: {}", r.name),
            Some(_) => info!("interior: (unnamed)"),
            None => info!("interior: outside"),
        }
        current.0 = audio;
    }
}
