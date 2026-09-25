//! World of Warcraft 1.12.1 sound as the client picks and schedules it, mixed by kira.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod anim_events;
mod combat;
mod config;
mod emitter_pool;
mod footsteps;
mod health;
mod interior;
pub mod kit;
mod limiter;
mod liquid_loop;
mod log;
pub mod math;
mod meter;
mod mix_tap;
pub mod mixer;
mod output;
mod plugin;
mod reverb;
pub mod tables;
mod water;
mod zone;

pub use config::SoundConfig;
pub use footsteps::SoundBody;
pub use interior::{CurrentInterior, InteriorAudio};
pub use kit::{KitRef, Played, SoundCategory, SoundKits, play_kit};
pub use liquid_loop::{LIQUID_LOOP_REACH, Listening};
pub use mixer::{Mixer, MixerSettings};
pub use output::{OFFLINE_SAMPLE_RATE, Output};
pub use plugin::{AudioListener, ListenerCharacter, SoundOutput, SoundPlugin, SoundSystems};

/// The crate's own debug tests abort on an allocation inside the output's realtime scopes; a binary
/// that plays sound declares the same allocator.
#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
