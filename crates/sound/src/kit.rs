//! The kit player: what the client does between "play this `SoundEntries` kit" and the mixer. It
//! gates on distance, on the kit's voice bus and duplicates, picks a variation from a depleting
//! pool, varies the shot's volume and pitch, and keeps each playing channel's gain on its
//! category, rolloff and near field.

mod pump;
#[cfg(test)]
mod tests;
mod voice;

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use bevy::prelude::*;
use mpq::Chain;

pub use pump::pump_channels;
pub use voice::Bus;

use crate::config::SoundConfig;
use crate::mixer::{self, StaticSoundData};
use crate::tables::{Kit, KitCatalog, kit_flags};
use crate::{SoundOutput, math};

/// Which slider scales a channel. The caller decides, by which of the client's play drivers it
/// is; a kit's own type never does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoundCategory {
    Sfx,
    Music,
    Ambience,
}

impl SoundCategory {
    fn name(self) -> &'static str {
        match self {
            Self::Sfx => "sfx",
            Self::Music => "music",
            Self::Ambience => "ambience",
        }
    }
}

/// A kit's variation weights still to play: a pick spends one, and an empty pool refills. With
/// the data's usual weights of one, no variation repeats until every one has played.
struct PickState {
    remaining: Vec<u32>,
}

/// xorshift32. The client draws from its own engine generator; only the transform of the draw
/// is its behaviour.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// The kit table and what playing from it keeps: decoded files and the variation pools.
#[derive(Resource)]
pub struct SoundKits {
    catalog: KitCatalog,
    chain: Option<std::sync::Arc<Chain>>,
    cache: HashMap<String, StaticSoundData>,
    pick: HashMap<u32, PickState>,
    rng: Rng,
}

impl SoundKits {
    pub fn new(catalog: KitCatalog, chain: std::sync::Arc<Chain>) -> Self {
        Self {
            catalog,
            chain: Some(chain),
            cache: HashMap::new(),
            pick: HashMap::new(),
            rng: Rng(0x9e37_79b9),
        }
    }

    pub fn catalog(&self) -> &KitCatalog {
        &self.catalog
    }

    /// Every file of every kit comes out of the chain.
    pub(crate) fn read(&self, path: &str) -> Result<Vec<u8>> {
        let chain = self.chain.as_ref().context("no install to read from")?;
        chain.read(path).with_context(|| format!("reading {path}"))
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            catalog: KitCatalog::default(),
            chain: None,
            cache: HashMap::new(),
            pick: HashMap::new(),
            rng: Rng(0x9e37_79b9),
        }
    }

    fn pick_variation(&mut self, kit: u32, weights: &[u32]) -> usize {
        if weights.len() == 1 {
            return 0;
        }
        let st = self.pick.entry(kit).or_insert_with(|| PickState {
            remaining: weights.to_vec(),
        });
        let mut total: u32 = st.remaining.iter().sum();
        if total == 0 {
            st.remaining.copy_from_slice(weights);
            total = st.remaining.iter().sum();
        }
        if total == 0 {
            return 0;
        }
        let r = self.rng.next() % total;
        let mut acc = 0u32;
        for (i, w) in st.remaining.iter().enumerate() {
            acc += w;
            if acc > r {
                st.remaining[i] -= 1;
                return i;
            }
        }
        st.remaining.len() - 1
    }

    fn sfx(&mut self, path: &str) -> Result<StaticSoundData> {
        let key = path.to_ascii_lowercase();
        if let Some(d) = self.cache.get(&key) {
            return Ok(d.clone());
        }
        let data = mixer::sfx_from_bytes(self.read(path)?)?;
        self.cache.insert(key, data.clone());
        Ok(data)
    }
}

/// One playing channel.
pub(crate) struct ActiveChannel {
    pub(crate) kit: u32,
    /// The entity the channel belongs to: it follows a looping one, and a despawn stops it.
    source: Option<Entity>,
    tracked: bool,
    handle: mixer::StaticSoundHandle,
    /// The spatial track keeping a 3-D voice alive; `None` is 2-D.
    track: Option<mixer::SpatialTrackHandle>,
    pos: Option<Vec3>,
    min_dist: f32,
    cutoff: f32,
    /// The shot's own volume, base plus variation.
    v: f32,
    /// A driver-animated gain: the fade lane of loops whose volume the pump owns.
    gain: f32,
    category: SoundCategory,
    pub(crate) bus: Bus,
    /// A loop is a bed, and the voice cap never steals one.
    looping: bool,
    /// The last effective amplitude the pump fed, which the voice cap ranks channels by.
    amp: f32,
}

/// The rest of the client's play call, beyond which kit, where and in which category.
#[derive(Clone, Copy, Default)]
pub(crate) struct PlayExtras {
    pub(crate) source: Option<Entity>,
    /// Loop whatever the kit's own flag says: the drivers whose column is the loop authority.
    pub(crate) force_loop: bool,
    /// Skip the one-shot lane's duplicate suppressors, for a caller that keeps one channel per
    /// kit by construction.
    pub(crate) dedupe_exempt: bool,
    pub(crate) bus: Bus,
}

/// A kit by id, or by its `PlaySoundByName` name.
pub enum KitRef<'a> {
    Id(u32),
    Name(&'a str),
}

/// What one play chose, whether or not it opened a channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Played {
    pub kit: u32,
    pub path: String,
    /// The amplitude the channel started at.
    pub amp: f32,
    /// The playback rate the pitch variation set; `1.0` without one.
    pub rate: f64,
    pub looping: bool,
}

/// Resolve, gate, pick, decode, play. `pos: None` plays in 2-D. `Ok(false)` is a gate refusing,
/// which the client did not treat as an error either.
pub fn play_kit(
    kits: &mut SoundKits,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit_ref: KitRef<'_>,
    pos: Option<Vec3>,
    category: SoundCategory,
) -> Result<bool> {
    play_kit_ext(
        kits,
        out,
        config,
        listener,
        kit_ref,
        pos,
        category,
        PlayExtras::default(),
    )
}

/// [`play_kit`] with the rest of the client's play call.
#[allow(clippy::too_many_arguments)]
pub(crate) fn play_kit_ext(
    kits: &mut SoundKits,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit_ref: KitRef<'_>,
    pos: Option<Vec3>,
    category: SoundCategory,
    extras: PlayExtras,
) -> Result<bool> {
    let kit = match kit_ref {
        KitRef::Id(id) => kits.catalog.get(id),
        KitRef::Name(name) => kits.catalog.by_name(name),
    }
    .ok_or_else(|| anyhow!("unknown sound kit"))?;
    let Kit {
        id,
        volume,
        flags,
        min_distance: min_dist,
        distance_cutoff: cutoff,
        eax_def,
        ..
    } = *kit;
    let d_sq = pos.map(|p| math::dist_sq(listener, p));
    if let Some(d_sq) = d_sq
        && cutoff > 0.0
        && !math::audible(d_sq, cutoff)
    {
        return Ok(false);
    }
    let weights: Vec<u32> = kit.files.iter().map(|(_, w)| *w).collect();
    if weights.is_empty() || turned_away(out, id, flags, extras) {
        return Ok(false);
    }
    let pick = kits.pick_variation(id, &weights);
    let path = kits.catalog.get(id).map(|k| k.files[pick].0.clone());
    let path = path.context("the kit resolved above")?;
    let data = kits.sfx(&path)?;
    let v = if flags & kit_flags::VARY_VOLUME == 0 {
        math::variation_volume(None, volume, 1.0)
    } else {
        let draw = math::variation_draw(kits.rng.next());
        math::variation_volume(Some(draw), volume, 1.0)
    };
    let atten = d_sq.map_or(1.0, |d| {
        math::fmod_rolloff(d, min_dist) * near_field(d, cutoff)
    });
    let amp = config.category_amp(category) * v * atten;
    let mut data = data.volume(mixer::amp_to_db(amp));
    let mut rate = 1.0;
    if flags & kit_flags::VARY_PITCH != 0 {
        let draw = math::variation_draw(kits.rng.next());
        rate = f64::from(math::variation_pitch_freq(draw)) / f64::from(data.sample_rate);
        data = data.playback_rate(rate);
    }
    let looping = extras.force_loop || flags & kit_flags::LOOPING != 0;
    if looping {
        data = data.loop_region(..);
    }
    // Last, because it is the one gate that can stop another sound.
    if !voice::claim_voice(out, amp) {
        return Ok(false);
    }
    let Some(mixer) = out.mixer.as_mut() else {
        return Ok(false);
    };
    let (track, handle) = match pos {
        // A kit with no `SoundSamplePreferences` row is dry however wet the zone is.
        Some(p) => {
            let (t, h) = mixer.play_3d(data, p, eax_def != 0)?;
            (Some(t), h)
        }
        None => (None, mixer.play_2d(data)?),
    };
    let name = kits.catalog.get(id).map_or("?", |k| k.name.as_str());
    let spatial = match pos {
        Some(_) if eax_def != 0 => "3d wet",
        Some(_) => "3d dry",
        None => "2d",
    };
    debug!(
        "sound: play kit {id} ({name}) {} {spatial}",
        category.name()
    );
    let played = Played {
        kit: id,
        path,
        amp,
        rate,
        looping,
    };
    out.note_play(&played, category.name(), spatial, pos);
    out.channels.push(ActiveChannel {
        kit: id,
        source: extras.source,
        tracked: looping && extras.source.is_some(),
        handle,
        track,
        pos,
        min_dist,
        cutoff,
        v,
        gain: 1.0,
        category,
        bus: extras.bus,
        looping,
        amp,
    });
    Ok(true)
}

/// Whether the bus cap, the kit's no-duplicates flag or the same-kit cap turns a play away.
fn turned_away(out: &mut SoundOutput, id: u32, flags: u32, extras: PlayExtras) -> bool {
    if voice::bus_at_cap(out.channels.iter().map(|c| c.bus), extras.bus) {
        return true;
    }
    let live_same_kit = out.channels.iter().filter(|c| c.kit == id).count();
    if voice::no_duplicates_blocks(extras.dedupe_exempt, flags, live_same_kit) {
        return true;
    }
    if voice::same_kit_cap_blocks(extras.dedupe_exempt, live_same_kit) {
        out.copies_dropped += 1;
        return true;
    }
    false
}

fn near_field(d_sq: f32, cutoff: f32) -> f32 {
    if cutoff > 0.0 {
        math::near_field_atten(d_sq, cutoff)
    } else {
        1.0
    }
}
