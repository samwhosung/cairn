//! World of Warcraft 1.12.1 sound as the client picks and schedules it, mixed by kira.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod config;
mod health;
pub mod kit;
mod limiter;
mod log;
pub mod math;
mod meter;
mod mix_tap;
pub mod mixer;
mod output;
mod plugin;
pub mod tables;

pub use config::SoundConfig;
pub use kit::{KitRef, Played, SoundCategory, SoundKits, play_kit};
pub use mixer::{Mixer, MixerSettings};
pub use output::{OFFLINE_SAMPLE_RATE, Output};
pub use plugin::{AudioListener, ListenerCharacter, SoundOutput, SoundPlugin, SoundSystems};

/// A debug build aborts on an allocation inside the output's realtime scopes.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
