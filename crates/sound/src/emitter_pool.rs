//! The ambient emitter pool: the client voices sound ids, not doodads. A doodad's `$DSL` key
//! registers its position as one record in the entry holding that id; each entry runs one channel,
//! at whichever of its records is nearest the listener, moved rather than restarted as that
//! changes. Of the 32 entries the first four, in claim order and not by distance, may sound at
//! once; a fifth fades out over 3 s. An entry holds up to 256 records.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::config::SoundConfig;
use crate::kit::{
    KitRef, PlayExtras, SoundCategory, SoundKits, kit_name, play_kit_ext, set_source_kit_gain,
    source_kit_playing, stop_source,
};
use crate::{AudioListener, SoundOutput, math};

const POOL_ENTRIES: usize = 32;
const RECORDS_PER_ENTRY: usize = 256;
const PLAYING_CAP: usize = 4;
const FADE_SECS: f32 = 3.0;

/// The entity an entry's channel rides; moving it is the reposition.
#[derive(Component)]
pub(crate) struct PoolEmitter;

struct Record {
    owner: Entity,
    pos: Vec3,
}

#[derive(Default)]
struct Entry {
    /// The `SoundEntries` id held; 0 is free.
    id: u32,
    /// The kit resolves to no row. The client negates the id instead, which makes the owner
    /// re-register every cycle; flagging it keeps the owner where it is, silent either way.
    failed: bool,
    /// In claim order.
    records: Vec<Record>,
    voice: Option<Entity>,
}

impl Entry {
    fn entitled(&self) -> bool {
        self.id != 0 && !self.failed && !self.records.is_empty()
    }

    /// A tie keeps the first record.
    fn nearest(&self, listener: Vec3) -> Option<Vec3> {
        self.records
            .iter()
            .map(|r| (math::dist_sq(listener, r.pos), r.pos))
            .reduce(|best, cand| if cand.0 < best.0 { cand } else { best })
            .map(|(_, pos)| pos)
    }
}

/// A channel let go, fading out: the entry is free to claim again while it dies.
struct Fading {
    emitter: Entity,
    kit: u32,
    gain: f32,
}

#[derive(Resource)]
pub(crate) struct AmbientEmitterPool {
    entries: [Entry; POOL_ENTRIES],
    /// Each owner's entry: one registration per owner.
    handles: EntityHashMap<usize>,
    fading: Vec<Fading>,
    complained: std::collections::HashSet<u32>,
    last_census: Vec<(u32, bool)>,
    last_withheld: Vec<u32>,
}

impl Default for AmbientEmitterPool {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| Entry::default()),
            handles: EntityHashMap::default(),
            fading: Vec::new(),
            complained: std::collections::HashSet::new(),
            last_census: Vec::new(),
            last_withheld: Vec::new(),
        }
    }
}

impl AmbientEmitterPool {
    /// The same id again only moves the owner's record; another id releases it and registers
    /// anew, so two keys on one sequence alternate through one registration.
    pub(crate) fn register(&mut self, owner: Entity, id: u32, pos: Vec3, listener: Vec3) {
        if let Some(&e) = self.handles.get(&owner) {
            if self.entries[e].id == id {
                self.reposition(e, owner, pos);
                return;
            }
            self.release(owner);
        }
        let Some(e) = self
            .entries
            .iter()
            .position(|x| x.id == id)
            .or_else(|| self.entries.iter().position(|x| x.id == 0))
        else {
            return;
        };
        if self.entries[e].id == 0 {
            self.entries[e].id = id;
            self.entries[e].failed = false;
        }
        if self.entries[e].records.len() >= RECORDS_PER_ENTRY {
            // The first record farther than the newcomer goes, in claim order; none farther
            // rejects the newcomer.
            let d = math::dist_sq(listener, pos);
            let Some(victim) = self.entries[e]
                .records
                .iter()
                .position(|r| math::dist_sq(listener, r.pos) > d)
            else {
                return;
            };
            let gone = self.entries[e].records.remove(victim);
            self.handles.remove(&gone.owner);
        }
        self.entries[e].records.push(Record { owner, pos });
        self.handles.insert(owner, e);
    }

    fn reposition(&mut self, e: usize, owner: Entity, pos: Vec3) {
        if let Some(r) = self.entries[e]
            .records
            .iter_mut()
            .find(|r| r.owner == owner)
        {
            r.pos = pos;
        }
    }

    /// The owner's last record going frees the entry, and its channel fades out.
    pub(crate) fn release(&mut self, owner: Entity) {
        let Some(e) = self.handles.remove(&owner) else {
            return;
        };
        let entry = &mut self.entries[e];
        entry.records.retain(|r| r.owner != owner);
        if !entry.records.is_empty() {
            return;
        }
        let (kit, voice) = (entry.id, entry.voice);
        *entry = Entry::default();
        if let Some(emitter) = voice {
            self.fading.push(Fading {
                emitter,
                kit,
                gain: 1.0,
            });
        }
    }

    /// Marks each entry whose kit has no `SoundEntries` row as failed, so it never sounds.
    fn fail_unknown_kits(&mut self, kits: &SoundKits) {
        for entry in &mut self.entries {
            if entry.id != 0 && !entry.failed && kit_name(kits, entry.id).is_none() {
                entry.failed = true;
                if self.complained.insert(entry.id) {
                    warn!(
                        "doodad emitter kit {}: no SoundEntries row, silent",
                        entry.id
                    );
                }
            }
        }
    }

    /// Fades the entry's channel and keeps everything else, to sound again under the cap.
    fn retire(&mut self, e: usize) {
        let (kit, voice) = {
            let entry = &mut self.entries[e];
            (entry.id, entry.voice.take())
        };
        if let Some(emitter) = voice {
            self.fading.push(Fading {
                emitter,
                kit,
                gain: 1.0,
            });
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CapStep {
    Skip,
    Service,
    Retire,
}

/// The count is of entries that came back live and in range: an entitled entry past its kit's
/// cutoff, or whose start failed, holds none of the four.
const fn cap_step(entitled: bool, sounding_so_far: usize) -> CapStep {
    if !entitled {
        CapStep::Skip
    } else if sounding_so_far >= PLAYING_CAP {
        CapStep::Retire
    } else {
        CapStep::Service
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pump_emitters(
    mut pool: ResMut<'_, AmbientEmitterPool>,
    mut emitters: Query<'_, '_, &mut Transform, With<PoolEmitter>>,
    time: Res<'_, Time>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
    mut commands: Commands<'_, '_>,
) {
    let Some(mut kits) = kits else {
        return;
    };
    if out.mixer.is_none() {
        return;
    }
    let listener_pos = listener.pos;
    let step = time.delta_secs() / FADE_SECS;
    let pool = &mut *pool;
    pool.fading.retain_mut(|f| {
        f.gain -= step;
        if f.gain <= 0.0 || !source_kit_playing(&out, f.emitter, f.kit) {
            stop_source(&mut out, f.emitter);
            commands.entity(f.emitter).despawn();
            return false;
        }
        set_source_kit_gain(&mut out, f.emitter, f.kit, f.gain);
        true
    });
    pool.fail_unknown_kits(&kits);
    let mut sounding = 0usize;
    let mut census: Vec<(u32, bool)> = Vec::new();
    let mut withheld: Vec<u32> = Vec::new();
    for e in 0..POOL_ENTRIES {
        match cap_step(pool.entries[e].entitled(), sounding) {
            CapStep::Skip => {
                pool.retire(e);
                continue;
            }
            CapStep::Retire => {
                withheld.push(pool.entries[e].id);
                pool.retire(e);
                continue;
            }
            CapStep::Service => {}
        }
        let id = pool.entries[e].id;
        let Some(nearest) = pool.entries[e].nearest(listener_pos) else {
            continue;
        };
        let emitter = if let Some(em) = pool.entries[e].voice {
            if let Ok(mut tf) = emitters.get_mut(em)
                && tf.translation != nearest
            {
                tf.translation = nearest;
            }
            em
        } else {
            let em = commands
                .spawn((PoolEmitter, Transform::from_translation(nearest)))
                .id();
            pool.entries[e].voice = Some(em);
            em
        };
        if source_kit_playing(&out, emitter, id) {
            sounding += 1;
            census.push((id, true));
            continue;
        }
        // Past the kit's cutoff the channel was stopped rather than left at zero gain, so it
        // restarts here; a hum out of earshot loses only its phase.
        if let Err(err) = play_kit_ext(
            &mut kits,
            &mut out,
            &config,
            listener_pos,
            KitRef::Id(id),
            Some(nearest),
            SoundCategory::Sfx,
            PlayExtras {
                source: Some(emitter),
                force_loop: true,
                dedupe_exempt: true,
                ..PlayExtras::default()
            },
        ) && pool.complained.insert(id)
        {
            warn!("doodad emitter kit {id}: {err:#}");
        }
        let live = source_kit_playing(&out, emitter, id);
        if live {
            sounding += 1;
        }
        census.push((id, live));
    }
    if census != pool.last_census || withheld != pool.last_withheld {
        debug!(
            "emitter pool: serviced {census:?}, withheld by the cap {withheld:?}, {} fading",
            pool.fading.len()
        );
        pool.last_census = census;
        pool.last_withheld = withheld;
    }
}

/// A doodad gone releases its record; the id sounds on while any other names it.
pub(crate) fn release_on_despawn(
    mut hosts: RemovedComponents<'_, '_, world::doodad_sound::SoundHost>,
    mut pool: ResMut<'_, AmbientEmitterPool>,
) {
    for entity in hosts.read() {
        pool.release(entity);
    }
}

#[cfg(test)]
mod tests;
