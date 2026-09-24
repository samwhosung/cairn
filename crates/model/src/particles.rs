use std::io::Cursor;

use m2::{M2ScalarTrack, parse_m2};

use crate::emit_timing::{EmitParams, EmitTiming};
use crate::key_anim::SeqSlot;
use crate::particle_curves::{CellRamp, OverLife, SplineData};
use crate::{le_f32, le_u16, le_u32};

const EMITTERS: usize = 0x13c;
const EMITTER_SIZE: usize = 0x1f8;
const MAX_EMITTERS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleShape {
    Plane,
    Sphere,
    Spline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleBlend {
    /// No depth write.
    Add,
    Alpha,
    /// Blending off, depth write on, fragments whose alpha (texel times particle) is under
    /// 224/255 discarded.
    AlphaKey,
    /// Blending off, depth write on, no alpha test.
    Opaque,
}

#[derive(Debug, Clone)]
pub struct ParticleEmitterDef {
    pub flags: u32,
    pub position: [f32; 3],
    pub bone: u16,
    pub shape: ParticleShape,
    pub blend: ParticleBlend,
    /// Whether the scene's light shades the quads.
    pub lit: bool,
    /// A model each particle draws as, in place of a quad.
    pub geometry_model: Option<String>,
    /// A model whose own emitters, up to four, emit from every live particle.
    pub recursion_model: Option<String>,
    pub texture: Option<String>,
    /// The flipbook atlas; both are powers of two, `1×1` when the file's are not.
    pub tile_rows: u16,
    pub tile_cols: u16,
    /// `0` a head quad, `1` a tail streak, `2` both.
    pub head_tail: u8,
    pub timing: EmitTiming,
    pub params: EmitParams,
    /// Each frame velocity loses `min(dt · drag, 1)` of itself.
    pub drag: f32,
    /// Seconds: a tail streak is `speed · tail_time` long.
    pub tail_time: f32,
    /// A geometry particle's spin range, radians a second. The client reads only X as
    /// `min + u · (max − min)`; Y and Z are a draw in `1..2` times `max − min`.
    pub angular_velocity_min: [f32; 3],
    pub angular_velocity_max: [f32; 3],
    /// Scales the emitter velocity births inherit under flag `0x40`.
    pub inherit_scale: f32,
    /// Two `(emitter speed, fraction)` samples of the flag `0x4000` follow response.
    pub follow_speed1: f32,
    pub follow_scale1: f32,
    pub follow_speed2: f32,
    pub follow_scale2: f32,
    /// A particle's size flickers by noise at `twinkle_speed · age`; see [`Self::twinkle`].
    pub twinkle_speed: f32,
    /// Below 1, the share of frames a particle draws at all.
    pub twinkle_percent: f32,
    pub twinkle_min: f32,
    pub twinkle_max: f32,
    /// `Some` for a spline emitter whose chain fits the file.
    pub spline: Option<SplineData>,
    /// Radians a second a quad turns in its plane; a negative spin turns half the particles
    /// the other way.
    pub spin: f32,
    pub over_life: OverLife,
}

impl ParticleEmitterDef {
    /// Flag `0x10`: particles ride the emitter; clear, they stay where they were born but for
    /// what [`Self::follow_emitter`] moves them.
    pub fn model_space(&self) -> bool {
        self.flags & 0x10 != 0
    }

    /// Flag `0x4000`: live particles follow [`Self::follow_line`]'s share of the emitter's
    /// motion each frame.
    pub fn follow_emitter(&self) -> bool {
        self.flags & 0x4000 != 0
    }

    /// `(slope, intercept)` of the line through the two follow samples, the fraction being
    /// `clamp(slope · speed + intercept, 0, 1)`; `None` when the speeds coincide.
    pub fn follow_line(&self) -> Option<(f32, f32)> {
        ((self.follow_speed2 - self.follow_speed1).abs() >= 1e-6).then(|| {
            let slope = (self.follow_scale2 - self.follow_scale1)
                / (self.follow_speed2 - self.follow_speed1);
            (slope, self.follow_scale1 - slope * self.follow_speed1)
        })
    }

    /// Flag `0x40`: births add the emitter's recent motion to their velocity.
    pub fn inherits_emitter_motion(&self) -> bool {
        self.flags & 0x40 != 0
    }

    pub fn scale_size_by_instance(&self) -> bool {
        self.flags & 0x20 != 0
    }

    /// Flag `0x100`: a sphere's births fly straight up.
    pub fn sphere_up(&self) -> bool {
        self.flags & 0x100 != 0
    }

    /// Flag `0x80` on a sphere: a particle dies the frame it moves away from the emitter.
    pub fn kill_outbound(&self) -> bool {
        self.shape == ParticleShape::Sphere && self.flags & 0x80 != 0
    }

    pub fn tail_clamps_to_age(&self) -> bool {
        self.flags & 0x400 != 0
    }

    /// Flag `0x200`: each spin axis of a geometry particle flips sign by a coin toss.
    pub fn tumble_random_sign(&self) -> bool {
        self.flags & 0x200 != 0
    }

    /// Flag `0x2000`: a birth stored in the world (flag `0x10` clear) drops onto whatever lies
    /// within 20 yards below it, raised by its half-extent.
    pub fn ground_snap(&self) -> bool {
        self.flags & 0x2000 != 0
    }

    /// Flag `0x8000`: the emitter fires `rate` particles at once when its gate opens, instead of
    /// pouring them.
    pub fn burst(&self) -> bool {
        self.flags & 0x8000 != 0
    }

    /// Flag `0x1000`: the head quad lies in the emitter's XY plane instead of facing the camera.
    pub fn xy_quad(&self) -> bool {
        self.flags & 0x1000 != 0
    }

    /// The size multiplier for a twinkle `noise` in `0..1`; 1 when min and max coincide.
    pub fn twinkle(&self, noise: f32) -> f32 {
        if (self.twinkle_max - self.twinkle_min).abs() < 1e-6 {
            1.0
        } else {
            noise * (self.twinkle_max - self.twinkle_min) + self.twinkle_min
        }
    }
}

fn emitter_table(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() < EMITTERS + 8 || &bytes[..4] != b"MD20" {
        return None;
    }
    let count = le_u32(bytes, EMITTERS) as usize;
    let base = le_u32(bytes, EMITTERS + 4) as usize;
    (count != 0 && count <= MAX_EMITTERS && base + count * EMITTER_SIZE <= bytes.len())
        .then_some((base, count))
}

pub(crate) fn emitter_bones(bytes: &[u8]) -> Vec<(u16, u32)> {
    let Some((base, count)) = emitter_table(bytes) else {
        return Vec::new();
    };
    (0..count)
        .map(|i| {
            let e = base + i * EMITTER_SIZE;
            (le_u16(bytes, e + 0x14), le_u32(bytes, e + 0x04))
        })
        .collect()
}

fn le_vec3(b: &[u8], o: usize) -> [f32; 3] {
    [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
}

fn raw_track(
    b: &[u8],
    track: usize,
    elem: usize,
    read: impl Fn(&[u8], usize) -> f32,
) -> M2ScalarTrack {
    let mut out = M2ScalarTrack {
        gseq: 0xffff,
        ..M2ScalarTrack::default()
    };
    if track + 0x1c > b.len() {
        return out;
    }
    out.interp = le_u16(b, track);
    out.gseq = le_u16(b, track + 2);
    let (rn, ro) = (le_u32(b, track + 4) as usize, le_u32(b, track + 8) as usize);
    let (tn, to) = (
        le_u32(b, track + 0x0c) as usize,
        le_u32(b, track + 0x10) as usize,
    );
    let (vn, vo) = (
        le_u32(b, track + 0x14) as usize,
        le_u32(b, track + 0x18) as usize,
    );
    if ro + rn * 8 <= b.len() {
        out.ranges = (0..rn)
            .map(|i| (le_u32(b, ro + i * 8), le_u32(b, ro + i * 8 + 4)))
            .collect();
    }
    let n = tn.min(vn);
    if n > 0 && to + n * 4 <= b.len() && vo + n * elem <= b.len() {
        out.keys = (0..n)
            .map(|i| (le_u32(b, to + i * 4), read(b, vo + i * elem)))
            .collect();
    }
    out
}

fn blend_of(v: u8) -> ParticleBlend {
    match v {
        3 | 4 => ParticleBlend::Add,
        2 | 5 | 6 => ParticleBlend::Alpha,
        1 => ParticleBlend::AlphaKey,
        _ => ParticleBlend::Opaque,
    }
}

/// Which raw blend modes the client lights: the two multiplies light nothing.
const LIGHTING_BY_BLEND: [bool; 7] = [true, true, true, true, true, false, false];

const UNLIT: u32 = 0x1;

fn lit_of(flags: u32, blend_byte: u8) -> bool {
    flags & UNLIT == 0
        && LIGHTING_BY_BLEND
            .get(usize::from(blend_byte))
            .copied()
            .unwrap_or(true)
}

fn shape_of(v: u16) -> ParticleShape {
    match v {
        2 => ParticleShape::Sphere,
        3 => ParticleShape::Spline,
        _ => ParticleShape::Plane,
    }
}

pub(crate) fn texture_names(bytes: &[u8]) -> Vec<Option<String>> {
    match parse_m2(&mut Cursor::new(bytes)) {
        Ok(fmt) => fmt
            .model()
            .textures
            .iter()
            .map(|t| {
                let f = t
                    .filename
                    .string
                    .to_string_lossy()
                    .trim_end_matches('\0')
                    .to_string();
                (!f.is_empty()).then_some(f)
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// One slot spanning the whole timeline for a model without sequences.
fn seq_slots(bytes: &[u8]) -> Vec<SeqSlot> {
    let (n, o) = (le_u32(bytes, 0x1c) as usize, le_u32(bytes, 0x20) as usize);
    let mut slots: Vec<SeqSlot> = (0..n)
        .map_while(|i| {
            let e = o + i * 0x44;
            (e + 0x44 <= bytes.len()).then(|| SeqSlot {
                file_index: i,
                band_ms: (le_u32(bytes, e + 0x04), le_u32(bytes, e + 0x08)),
                looping: le_u32(bytes, e + 0x10) & 1 == 0,
            })
        })
        .collect();
    if slots.is_empty() {
        slots.push(SeqSlot {
            file_index: 0,
            band_ms: (0, u32::MAX),
            looping: true,
        });
    }
    slots
}

fn global_sequences(bytes: &[u8]) -> Vec<u32> {
    let (n, o) = (le_u32(bytes, 0x14) as usize, le_u32(bytes, 0x18) as usize);
    (0..n)
        .map_while(|i| (o + i * 4 + 4 <= bytes.len()).then(|| le_u32(bytes, o + i * 4)))
        .collect()
}

fn rgba_from_bgra(bytes: &[u8], o: usize) -> [f32; 4] {
    let v = le_u32(bytes, o);
    [
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
        ((v >> 24) & 0xff) as f32 / 255.0,
    ]
}

fn model_path_at(bytes: &[u8], e: usize, at: usize) -> Option<String> {
    let n = le_u32(bytes, e + at) as usize;
    let ofs = le_u32(bytes, e + at + 4) as usize;
    if n < 2 || ofs + n > bytes.len() {
        return None;
    }
    let s = String::from_utf8_lossy(&bytes[ofs..ofs + n])
        .trim_end_matches('\0')
        .to_string();
    (!s.is_empty()).then_some(s)
}

/// The M2's particle emitters; none when the file has no table that fits.
pub fn parse_m2_particle_emitters(bytes: &[u8]) -> Vec<ParticleEmitterDef> {
    let textures = texture_names(bytes);
    let Some((base, count)) = emitter_table(bytes) else {
        return Vec::new();
    };
    let slots = seq_slots(bytes);
    let gseq = global_sequences(bytes);
    (0..count)
        .map(|i| parse_emitter(bytes, base + i * EMITTER_SIZE, &textures, &slots, &gseq))
        .collect()
}

fn parse_emitter(
    bytes: &[u8],
    e: usize,
    textures: &[Option<String>],
    slots: &[SeqSlot],
    gseq: &[u32],
) -> ParticleEmitterDef {
    let shape = shape_of(le_u16(bytes, e + 0x2a));
    let spline = (shape == ParticleShape::Spline)
        .then(|| {
            let q = le_u32(bytes, e + 0x1d4) as usize;
            let ofs = le_u32(bytes, e + 0x1d8) as usize;
            let n = 3 * (q / 3) + 1;
            if q < 3 || ofs + n * 12 > bytes.len() {
                return None;
            }
            SplineData::new((0..n).map(|p| le_vec3(bytes, ofs + p * 12)).collect())
        })
        .flatten();
    let tiles = match (le_u16(bytes, e + 0x30), le_u16(bytes, e + 0x32)) {
        (r, c) if r.is_power_of_two() && c.is_power_of_two() => (r, c),
        _ => (1, 1),
    };
    let over_life = OverLife {
        mid: le_f32(bytes, e + 0x14c),
        color: [
            rgba_from_bgra(bytes, e + 0x150),
            rgba_from_bgra(bytes, e + 0x154),
            rgba_from_bgra(bytes, e + 0x158),
        ],
        scale: [
            le_f32(bytes, e + 0x15c),
            le_f32(bytes, e + 0x160),
            le_f32(bytes, e + 0x164),
        ],
        head_cells: [
            CellRamp::new(le_u16(bytes, e + 0x168), le_u16(bytes, e + 0x16a)),
            CellRamp::new(le_u16(bytes, e + 0x16e), le_u16(bytes, e + 0x170)),
        ],
        tail_cells: [
            CellRamp::new(le_u16(bytes, e + 0x174), le_u16(bytes, e + 0x176)),
            CellRamp::new(le_u16(bytes, e + 0x178), le_u16(bytes, e + 0x17a)),
        ],
        repeat: [
            f32::from(le_u16(bytes, e + 0x16c)),
            f32::from(le_u16(bytes, e + 0x172)),
        ],
    };
    let track = |at: usize| raw_track(bytes, e + at, 4, le_f32);
    ParticleEmitterDef {
        flags: le_u32(bytes, e + 0x04),
        position: le_vec3(bytes, e + 0x08),
        bone: le_u16(bytes, e + 0x14),
        shape,
        spline,
        geometry_model: model_path_at(bytes, e, 0x18),
        recursion_model: model_path_at(bytes, e, 0x20),
        blend: blend_of(bytes[e + 0x28]),
        lit: lit_of(le_u32(bytes, e + 0x04), bytes[e + 0x28]),
        texture: textures
            .get(le_u16(bytes, e + 0x16) as usize)
            .cloned()
            .flatten(),
        tile_rows: tiles.0,
        tile_cols: tiles.1,
        head_tail: bytes[e + 0x2c],
        timing: EmitTiming::bake(
            &track(0xdc),
            &raw_track(bytes, e + 0x1dc, 1, |b, o| f32::from(b[o] != 0)),
            slots,
            gseq,
        ),
        params: EmitParams::bake(
            [
                &track(0x34),
                &track(0x50),
                &track(0x6c),
                &track(0x88),
                &track(0xa4),
                &track(0xc0),
                &track(0xf8),
                &track(0x114),
                &track(0x130),
            ],
            slots,
            gseq,
        ),
        drag: le_f32(bytes, e + 0x194),
        tail_time: le_f32(bytes, e + 0x17c),
        angular_velocity_min: le_vec3(bytes, e + 0x19c),
        angular_velocity_max: le_vec3(bytes, e + 0x1a8),
        inherit_scale: le_f32(bytes, e + 0x190),
        follow_speed1: le_f32(bytes, e + 0x1c4),
        follow_scale1: le_f32(bytes, e + 0x1c8),
        follow_speed2: le_f32(bytes, e + 0x1cc),
        follow_scale2: le_f32(bytes, e + 0x1d0),
        twinkle_speed: le_f32(bytes, e + 0x180),
        twinkle_percent: le_f32(bytes, e + 0x184),
        twinkle_min: le_f32(bytes, e + 0x188),
        twinkle_max: le_f32(bytes, e + 0x18c),
        spin: le_f32(bytes, e + 0x198),
        over_life,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_one_is_the_unlit_flag_and_the_multiplies_never_light() {
        assert!(lit_of(0x0002, 2));
        assert!(!lit_of(0x0029, 4));
        assert!(!lit_of(0x0021, 2));
        for blend in 0u8..=4 {
            assert!(lit_of(0x0000, blend));
        }
        assert!(!lit_of(0x0000, 5));
        assert!(!lit_of(0x0000, 6));
        assert!(lit_of(0x0000, 7), "past the table is lit");
    }
}
