use wowfile::capped;

use crate::{le_f32, le_u16, le_u32};

/// One bone's keyframes in one sequence, in raw WoW model space: `(seconds from the sequence
/// start, value)`, rotations as `[x, y, z, w]` quaternions. A channel without keys on the sequence
/// timeline is empty; multi-key global-sequence channels are in
/// [`parse_m2_global_sequence_bones`](crate::parse_m2_global_sequence_bones), and a bone in
/// neither holds its rest pose.
#[derive(Debug, Clone)]
pub struct BoneKeys {
    pub bone: u16,
    pub translation: Vec<(f32, [f32; 3])>,
    pub rotation: Vec<(f32, [f32; 4])>,
    pub scale: Vec<(f32, [f32; 3])>,
}

/// One event key: a 4CC trigger on a sequence's timeline.
#[derive(Debug, Clone, Copy)]
pub struct AnimEvent {
    /// Seconds from the sequence start, like the bone keys.
    pub time: f32,
    /// The identifier as it reads, `*b"$FL0"`.
    pub ident: [u8; 4],
    /// The record's payload (`+0x04`): a `SoundEntries` id for `$SND`, `$DSL` and `$DSO`.
    pub data: u32,
    /// The bone the record rides (`+0x08`), carried per key because a model can author one 4CC
    /// at several points.
    pub bone: u16,
    /// The record's point (`+0x0c`) in raw WoW model space, which the client moves with `bone`.
    pub position: [f32; 3],
}

/// One animation sequence: its `AnimationData.dbc` id, time band, playback fields, bone keys and
/// events. Pick sequences by `anim_id`: this list skips zero-length sequences, and a model's first
/// sequence need not be its Stand.
#[derive(Debug, Clone)]
pub struct ModelAnimation {
    /// `AnimationData.dbc` id, `0` for Stand.
    pub anim_id: u16,
    /// The sequence's index in the file, which indexes every track's per-sequence key windows. It
    /// differs from the index in this list once a zero-length sequence is skipped.
    pub seq_index: usize,
    /// The sequence's band on the model's shared keyframe timeline, in milliseconds.
    pub start_ms: u32,
    pub end_ms: u32,
    /// The band's length in seconds, always positive.
    pub duration: f32,
    /// Sequence flag bit 0 clear. When it is set the client plays the sequence its rolled replay
    /// count of times, then holds the last frame.
    pub looping: bool,
    /// The ground speed the sequence was animated at.
    pub move_speed: f32,
    /// Seconds the client takes to cross-fade into this sequence.
    pub blend_time: f32,
    /// The centre of the sequence's bounding box, raw WoW model space.
    pub bounds_center: [f32; 3],
    pub bounds_radius: f32,
    /// The sequence's bounding box, raw WoW model space.
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// This variation's weight when the client picks at random among the sequences that share
    /// `anim_id`.
    pub frequency: u16,
    /// The range of the play count the client rolls each time it arms the sequence.
    pub min_replay: u32,
    pub max_replay: u32,
    pub bones: Vec<BoneKeys>,
    /// The events inside the band, timed like the bone keys and sorted by time.
    pub events: Vec<AnimEvent>,
}

impl ModelAnimation {
    /// Whether every bone holds its rest pose for the whole band: at most one key per channel,
    /// each within `1e-4` of the identity (zero translation, `|w| = 1`, unit scale). Only bone
    /// keys are looked at; such a sequence can still carry events.
    pub fn is_rest_pose(&self) -> bool {
        const EPS: f32 = 1e-4;
        !self.bones.iter().any(|b| {
            b.translation.len() > 1
                || b.rotation.len() > 1
                || b.scale.len() > 1
                || b.translation
                    .iter()
                    .any(|(_, v)| v.iter().any(|c| c.abs() > EPS))
                || b.rotation
                    .iter()
                    .any(|(_, q)| (q[3].abs() - 1.0).abs() > EPS)
                || b.scale
                    .iter()
                    .any(|(_, s)| s.iter().any(|c| (c - 1.0).abs() > EPS))
        })
    }
}

struct ChannelTrack<T> {
    step: bool,
    gseq: u16,
    /// Each sequence's key-index window `(lo, hi)`, indexed by its file slot. When empty the
    /// client searches the whole key list.
    ranges: Vec<(u32, u32)>,
    keys: Vec<(u32, T)>,
}

type BoneChannels = (
    ChannelTrack<[f32; 3]>,
    ChannelTrack<[f32; 4]>,
    ChannelTrack<[f32; 3]>,
);

fn read_channel_track<T>(
    b: &[u8],
    track: usize,
    stride: usize,
    read_value: impl Fn(&[u8], usize) -> T,
) -> ChannelTrack<T> {
    if track + 0x1c > b.len() {
        return ChannelTrack {
            step: false,
            gseq: 0xffff,
            ranges: Vec::new(),
            keys: Vec::new(),
        };
    }
    let (rn, ro) = (
        le_u32(b, track + 0x04) as usize,
        le_u32(b, track + 0x08) as usize,
    );
    let (tn, to) = (
        le_u32(b, track + 0x0c) as usize,
        le_u32(b, track + 0x10) as usize,
    );
    let vo = le_u32(b, track + 0x18) as usize;
    let ranges = (0..rn)
        .map_while(|i| {
            let e = ro + i * 8;
            (e + 8 <= b.len()).then(|| (le_u32(b, e), le_u32(b, e + 4)))
        })
        .collect();
    let keys = (0..tn)
        .map_while(|k| {
            let (t_off, v_off) = (to + k * 4, vo + k * stride);
            (t_off + 4 <= b.len() && v_off + stride <= b.len())
                .then(|| (le_u32(b, t_off), read_value(b, v_off)))
        })
        .collect();
    ChannelTrack {
        step: le_u16(b, track) == 0,
        gseq: le_u16(b, track + 0x02),
        ranges,
        keys,
    }
}

impl<T: super::key_anim::Lerp + PartialEq> ChannelTrack<T> {
    fn band(&self, slot: usize, start: u32, end: u32) -> Vec<(f32, T)> {
        if self.keys.is_empty() {
            return Vec::new();
        }
        if self.gseq != 0xffff {
            return match self.keys.len() {
                1 => vec![(0.0, self.keys[0].1)],
                _ => Vec::new(),
            };
        }
        let last = self.keys.len() - 1;
        let (lo, hi) = self
            .ranges
            .get(slot)
            .map_or((0, last), |&(lo, hi)| (lo as usize, hi as usize));
        let at = |t| super::key_anim::sample_window(&self.keys, self.step, lo, hi, t);
        let rebase = |ts: u32| (ts.saturating_sub(start)) as f32 / 1000.0;
        let in_band: Vec<(u32, T)> = self
            .keys
            .iter()
            .copied()
            .filter(|&(ts, _)| ts >= start && ts <= end)
            .collect();
        let Some(&(first_ms, first_v)) = in_band.first() else {
            let Some(head) = at(start) else {
                return Vec::new();
            };
            return match at(end) {
                Some(tail) if tail != head => vec![(0.0, head), (rebase(end), tail)],
                _ => vec![(0.0, head)],
            };
        };
        let (last_ms, last_v) = in_band[in_band.len() - 1];
        let mut out = Vec::with_capacity(in_band.len() + 2);
        if first_ms > start
            && let Some(head) = at(start)
            && head != first_v
        {
            out.push((0.0, head));
        }
        out.extend(in_band.iter().map(|&(ts, v)| (rebase(ts), v)));
        if last_ms < end
            && let Some(tail) = at(end)
            && tail != last_v
        {
            out.push((rebase(end), tail));
        }
        out
    }
}

const HANDS_CLOSED_ANIM_ID: u16 = 15;

/// Each requested bone's rotation at the first frame of the model's `HandsClosed` sequence, as
/// `(bone, [x, y, z, w])`, sampled through the key window like the [`ModelAnimation`] clips.
/// Bones without a sequence-timeline rotation track are skipped; empty when the model has no
/// `HandsClosed` sequence.
pub fn hand_grip_finger_poses(bytes: &[u8], bones: &[u16]) -> Vec<(u16, [f32; 4])> {
    let b = bytes;
    if b.len() < 0x40 || &b[0..4] != b"MD20" {
        return Vec::new();
    }
    let (seq_count, seq_ofs) = (le_u32(b, 0x1c) as usize, le_u32(b, 0x20) as usize);
    let (bone_count, bone_ofs) = (le_u32(b, 0x34) as usize, le_u32(b, 0x38) as usize);
    let mut hands_closed = None;
    for s in 0..seq_count {
        let rec = seq_ofs + s * 0x44;
        if rec + 0x44 > b.len() {
            break;
        }
        if le_u16(b, rec) == HANDS_CLOSED_ANIM_ID {
            hands_closed = Some((s, le_u32(b, rec + 0x04)));
            break;
        }
    }
    let Some((slot, frame)) = hands_closed else {
        return Vec::new();
    };
    let quat = |b: &[u8], o: usize| {
        [
            le_f32(b, o),
            le_f32(b, o + 4),
            le_f32(b, o + 8),
            le_f32(b, o + 12),
        ]
    };
    let mut out = Vec::new();
    for &bone in bones {
        let bi = bone as usize;
        if bi >= bone_count || bone_ofs + bi * 0x6c + 0x6c > b.len() {
            continue;
        }
        let tr = read_channel_track(b, bone_ofs + bi * 0x6c + 0x28, 16, quat);
        if tr.gseq != 0xffff || tr.keys.is_empty() {
            continue;
        }
        let last = tr.keys.len() - 1;
        let (lo, hi) = tr
            .ranges
            .get(slot)
            .map_or((0, last), |&(lo, hi)| (lo as usize, hi as usize));
        let Some(v) = super::key_anim::sample_window(&tr.keys, tr.step, lo, hi, frame) else {
            continue;
        };
        out.push((bone, v));
    }
    out
}

struct EventRecord {
    ident: [u8; 4],
    data: u32,
    bone: u16,
    position: [f32; 3],
    times_ms: Vec<u32>,
}

/// Parse every sequence of an M2 into keyframes and events, in file order, skipping zero-length
/// sequences. Empty when `b` is under `0x40` bytes or not MD20.
///
/// Panics on an MD20 `b` shorter than `0x11c` bytes.
#[allow(clippy::too_many_lines)]
pub fn parse_m2_animations(b: &[u8]) -> Vec<ModelAnimation> {
    if b.len() < 0x40 || &b[0..4] != b"MD20" {
        return Vec::new();
    }
    let (seq_count, seq_ofs) = (le_u32(b, 0x1c) as usize, le_u32(b, 0x20) as usize);
    let (bone_count, bone_ofs) = (le_u32(b, 0x34) as usize, le_u32(b, 0x38) as usize);
    let vec3 = |b: &[u8], o: usize| [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)];
    let quat = |b: &[u8], o: usize| {
        [
            le_f32(b, o),
            le_f32(b, o + 4),
            le_f32(b, o + 8),
            le_f32(b, o + 12),
        ]
    };
    let (ev_count, ev_ofs) = (le_u32(b, 0x114) as usize, le_u32(b, 0x118) as usize);
    let mut model_events: Vec<EventRecord> =
        Vec::with_capacity(capped(ev_count, 44, b.len().saturating_sub(ev_ofs)));
    for e in 0..ev_count {
        let erec = ev_ofs + e * 44;
        if erec + 44 > b.len() {
            break;
        }
        let ident: [u8; 4] = match b.get(erec..erec + 4).and_then(|s| s.try_into().ok()) {
            Some(i) => i,
            None => break,
        };
        let data = le_u32(b, erec + 4);
        let bone = le_u32(b, erec + 8) as u16;
        let position = vec3(b, erec + 12);
        let (nts, ots) = (le_u32(b, erec + 36) as usize, le_u32(b, erec + 40) as usize);
        let mut times_ms = Vec::with_capacity(capped(nts, 4, b.len().saturating_sub(ots)));
        for t in 0..nts {
            let o = ots + t * 4;
            if o + 4 > b.len() {
                break;
            }
            times_ms.push(le_u32(b, o));
        }
        model_events.push(EventRecord {
            ident,
            data,
            bone,
            position,
            times_ms,
        });
    }

    let bone_tracks: Vec<BoneChannels> = (0..bone_count)
        .map_while(|i| {
            let brec = bone_ofs + i * 0x6c;
            (brec + 0x60 <= b.len()).then(|| {
                (
                    read_channel_track(b, brec + 0x0c, 12, vec3),
                    read_channel_track(b, brec + 0x28, 16, quat),
                    read_channel_track(b, brec + 0x44, 12, vec3),
                )
            })
        })
        .collect();

    let mut out = Vec::new();
    for s in 0..seq_count {
        let rec = seq_ofs + s * 0x44;
        if rec + 0x44 > b.len() {
            break;
        }
        let anim_id = le_u16(b, rec);
        let seq_index = s;
        let (start, end, flags) = (
            le_u32(b, rec + 0x04),
            le_u32(b, rec + 0x08),
            le_u32(b, rec + 0x10),
        );
        let move_speed = le_f32(b, rec + 0x0c);
        let blend_time = le_u32(b, rec + 0x20) as f32 / 1000.0;
        let (bmin, bmax) = (vec3(b, rec + 0x24), vec3(b, rec + 0x30));
        #[allow(clippy::manual_midpoint)]
        let bounds_center = [
            (bmin[0] + bmax[0]) * 0.5,
            (bmin[1] + bmax[1]) * 0.5,
            (bmin[2] + bmax[2]) * 0.5,
        ];
        let bounds_radius = le_f32(b, rec + 0x3c);
        let frequency = le_u16(b, rec + 0x14);
        let (min_replay, max_replay) = (le_u32(b, rec + 0x18), le_u32(b, rec + 0x1c));
        let duration = end.saturating_sub(start) as f32 / 1000.0;
        if duration <= 0.0 {
            continue;
        }
        let looping = flags & 1 == 0;
        let mut bones = Vec::new();
        for (i, tracks) in bone_tracks.iter().enumerate() {
            let (tr, rot, sc) = tracks;
            let translation = tr.band(seq_index, start, end);
            let rotation = rot.band(seq_index, start, end);
            let scale = sc.band(seq_index, start, end);
            if !(translation.is_empty() && rotation.is_empty() && scale.is_empty()) {
                bones.push(BoneKeys {
                    bone: i as u16,
                    translation,
                    rotation,
                    scale,
                });
            }
        }
        let mut events = Vec::new();
        for r in &model_events {
            for &ts in &r.times_ms {
                if ts >= start && ts <= end {
                    events.push(AnimEvent {
                        time: (ts - start) as f32 / 1000.0,
                        ident: r.ident,
                        data: r.data,
                        bone: r.bone,
                        position: r.position,
                    });
                }
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time));

        out.push(ModelAnimation {
            anim_id,
            seq_index,
            start_ms: start,
            end_ms: end,
            duration,
            looping,
            move_speed,
            blend_time,
            bounds_center,
            bounds_radius,
            bounds_min: bmin,
            bounds_max: bmax,
            frequency,
            min_replay,
            max_replay,
            bones,
            events,
        });
    }
    out
}
