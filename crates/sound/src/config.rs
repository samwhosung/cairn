use bevy::prelude::*;

use crate::kit::SoundCategory;

/// The player's sound settings; the volumes default to the client's, music 0.4 and ambience 0.6 of
/// full.
#[derive(Resource, Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct SoundConfig {
    /// Every sound: off, every category at zero and the master shut.
    pub enabled: bool,
    /// The main track only; selection and channels go on, so unmuting is instant.
    pub muted: bool,
    pub master: f32,
    pub sfx: f32,
    pub music: f32,
    pub ambience: f32,
    pub music_enabled: bool,
    pub ambience_enabled: bool,
    /// Keep sounding while the window is in the background; the client goes quiet.
    pub background_sound: bool,
    /// The zone's reverb preset, which the client plays only through EAX hardware.
    pub reverb: bool,
    /// The output limiter, which the client does not have.
    pub limiter: bool,
    /// The 3-D listener on the character, facing where the body faces; off, on the camera.
    pub listener_at_character: bool,
    /// Start a zone's next track the moment one ends, with no silence between.
    pub zone_music_no_delay: bool,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            muted: false,
            master: 1.0,
            sfx: 1.0,
            music: 0.4,
            ambience: 0.6,
            music_enabled: true,
            ambience_enabled: true,
            background_sound: false,
            reverb: false,
            limiter: true,
            listener_at_character: true,
            zone_music_no_delay: false,
        }
    }
}

impl SoundConfig {
    /// The slider a channel of `cat` multiplies in.
    pub fn category_amp(&self, cat: SoundCategory) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        match cat {
            SoundCategory::Sfx => self.sfx,
            SoundCategory::Music if self.music_enabled => self.music,
            SoundCategory::Ambience if self.ambience_enabled => self.ambience,
            _ => 0.0,
        }
    }
}
