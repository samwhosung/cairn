use std::io::Cursor;

use m2::parse_m2;
use mpq::Chain;

use crate::particles::emitter_bones;
use crate::{
    Error, le_u32, model_path, parse_m2_animations, parse_m2_global_sequence_bones,
    parse_m2_skeleton,
};

/// The texture-transform (UV animation) count from the MD20 header (`0x74`); 0 when `b` is under
/// `0x78` bytes or not MD20.
pub fn m2_texture_transform_count(b: &[u8]) -> usize {
    if b.len() < 0x78 || &b[0..4] != b"MD20" {
        return 0;
    }
    le_u32(b, 0x74) as usize
}

/// The ribbon emitter count from the MD20 header (`0x134`); 0 when `b` is under `0x138` bytes or
/// not MD20.
pub fn m2_ribbon_emitter_count(b: &[u8]) -> usize {
    if b.len() < 0x138 || &b[0..4] != b"MD20" {
        return 0;
    }
    le_u32(b, 0x134) as usize
}

/// A particle emitter's host bone, and whether that bone or any ancestor animates: motion
/// composes down the hierarchy, so either one moves the emitter.
#[derive(Debug, Clone, Copy)]
pub struct EmitterBoneLink {
    pub bone: u16,
    /// The emitter's flag word (`+0x04`), unparsed.
    pub flags: u32,
    /// A bone in the chain has a channel with more than one key in sequence 0.
    pub chain_seq0: bool,
    /// A bone in the chain has a global-sequence channel.
    pub chain_gseq: bool,
}

impl EmitterBoneLink {
    /// Whether the host chain animates at all.
    pub fn chain_animated(&self) -> bool {
        self.chain_seq0 || self.chain_gseq
    }
}

/// How much of an M2 animates. Sequence 0 is the first sequence [`parse_m2_animations`] returns:
/// the first in file order with a non-zero duration.
#[derive(Debug, Clone)]
pub struct M2AnimSummary {
    /// Sequences with a non-zero duration.
    pub sequence_count: usize,
    /// Sequence 0 has a bone channel with more than one key.
    pub seq0_has_bone_motion: bool,
    /// Bones with a channel of more than one key in sequence 0.
    pub seq0_animated_bone_count: usize,
    /// Sequences sharing sequence 0's `anim_id`, sequence 0 included.
    pub seq0_variation_count: usize,
    /// `(bone, "T" | "R" | "S", period_ms)` for every global-sequence channel.
    pub global_seq_channels: Vec<(u16, &'static str, u32)>,
    /// Colour alpha tracks as `(all, animated)`: animated means more than one key, not all equal.
    pub color_alpha_tracks: (usize, usize),
    /// Colour RGB tracks as `(all, with more than one key)`.
    pub color_rgb_tracks: (usize, usize),
    /// Transparency tracks as `(all, animated)`, counted like `color_alpha_tracks`.
    pub transparency_tracks: (usize, usize),
    /// See [`m2_texture_transform_count`].
    pub texture_transform_count: usize,
    /// `emitter_bones.len()`.
    pub particle_emitter_count: usize,
    /// One entry per particle emitter, in file order; empty when the table is over 256 entries or
    /// runs past the end of the file.
    pub emitter_bones: Vec<EmitterBoneLink>,
    /// See [`m2_ribbon_emitter_count`].
    pub ribbon_emitter_count: usize,
}

impl M2AnimSummary {
    /// Whether nothing animates: no sequence-0 bone motion, no global-sequence channel, no
    /// animated colour or transparency track, and no texture transform, particle or ribbon emitter.
    pub fn is_fully_static(&self) -> bool {
        !self.seq0_has_bone_motion
            && self.global_seq_channels.is_empty()
            && self.transparency_tracks.1 == 0
            && self.color_rgb_tracks.1 == 0
            && self.color_alpha_tracks.1 == 0
            && self.texture_transform_count == 0
            && self.particle_emitter_count == 0
            && self.ribbon_emitter_count == 0
    }
}

/// Summarise an M2's animation channels.
pub fn parse_m2_animation_summary(bytes: &[u8]) -> Result<M2AnimSummary, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    let model = format.model();

    let seqs = parse_m2_animations(bytes);
    let sequence_count = seqs.len();
    let seq0_variation_count = seqs.first().map_or(0, |s0| {
        seqs.iter().filter(|s| s.anim_id == s0.anim_id).count()
    });
    let seq0_animated_bone_count = seqs.first().map_or(0, |seq0| {
        seq0.bones
            .iter()
            .filter(|b| b.translation.len() > 1 || b.rotation.len() > 1 || b.scale.len() > 1)
            .count()
    });

    let mut global_seq_channels = Vec::new();
    for b in parse_m2_global_sequence_bones(bytes) {
        if let Some(t) = &b.translation {
            global_seq_channels.push((b.bone, "T", t.period_ms));
        }
        if let Some(r) = &b.rotation {
            global_seq_channels.push((b.bone, "R", r.period_ms));
        }
        if let Some(s) = &b.scale {
            global_seq_channels.push((b.bone, "S", s.period_ms));
        }
    }

    let seq0_bones: std::collections::HashSet<u16> = seqs
        .first()
        .map(|seq0| {
            seq0.bones
                .iter()
                .filter(|b| b.translation.len() > 1 || b.rotation.len() > 1 || b.scale.len() > 1)
                .map(|b| b.bone)
                .collect()
        })
        .unwrap_or_default();
    let gseq_bones: std::collections::HashSet<u16> =
        global_seq_channels.iter().map(|&(b, _, _)| b).collect();
    let skeleton = parse_m2_skeleton(bytes)?;
    let chain_flags = |bone: u16| -> (bool, bool) {
        let (mut seq0, mut gseq) = (false, false);
        let mut b = bone;
        for _ in 0..=skeleton.bones.len() {
            let Some(rec) = skeleton.bones.get(b as usize) else {
                break;
            };
            seq0 |= seq0_bones.contains(&b);
            gseq |= gseq_bones.contains(&b);
            if rec.parent < 0 {
                break;
            }
            b = rec.parent as u16;
        }
        (seq0, gseq)
    };
    let emitter_bones: Vec<EmitterBoneLink> = emitter_bones(bytes)
        .into_iter()
        .map(|(bone, flags)| {
            let (chain_seq0, chain_gseq) = chain_flags(bone);
            EmitterBoneLink {
                bone,
                flags,
                chain_seq0,
                chain_gseq,
            }
        })
        .collect();

    let count_animated = |tracks: &[m2::M2ScalarTrack]| -> (usize, usize) {
        (
            tracks.len(),
            tracks
                .iter()
                .filter(|t| t.keys.len() > 1 && t.constant().is_none())
                .count(),
        )
    };

    Ok(M2AnimSummary {
        sequence_count,
        seq0_has_bone_motion: seq0_animated_bone_count > 0,
        seq0_animated_bone_count,
        seq0_variation_count,
        global_seq_channels,
        color_alpha_tracks: count_animated(&model.color_alpha_tracks),
        transparency_tracks: count_animated(&model.transparency_tracks),
        color_rgb_tracks: (
            model.color_rgb_tracks.len(),
            model
                .color_rgb_tracks
                .iter()
                .filter(|t| t.keys.len() > 1)
                .count(),
        ),
        texture_transform_count: m2_texture_transform_count(bytes),
        particle_emitter_count: emitter_bones.len(),
        emitter_bones,
        ribbon_emitter_count: m2_ribbon_emitter_count(bytes),
    })
}

/// [`parse_m2_animation_summary`] for a model in `chain`; `.mdx` and `.mdl` paths resolve to `.m2`
/// as in [`load_m2_mesh`](crate::load_m2_mesh).
pub fn load_m2_animation_summary(chain: &Chain, raw_path: &str) -> Result<M2AnimSummary, Error> {
    let bytes = chain.read(&model_path(raw_path)).map_err(Error::Chain)?;
    parse_m2_animation_summary(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_with_counts(tex_transform_count: u32, ribbon_count: u32) -> Vec<u8> {
        let mut b = vec![0u8; 0x138];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&256u32.to_le_bytes());
        b[0x74..0x78].copy_from_slice(&tex_transform_count.to_le_bytes());
        b[0x134..0x138].copy_from_slice(&ribbon_count.to_le_bytes());
        b
    }

    #[test]
    fn texture_transform_and_ribbon_counts_read_the_header_offsets() {
        let b = header_with_counts(3, 7);
        assert_eq!(m2_texture_transform_count(&b), 3);
        assert_eq!(m2_ribbon_emitter_count(&b), 7);
    }

    #[test]
    fn texture_transform_and_ribbon_counts_are_zero_on_a_too_short_or_non_md20_buffer() {
        assert_eq!(m2_texture_transform_count(&[]), 0);
        assert_eq!(m2_ribbon_emitter_count(&[]), 0);
        let short = header_with_counts(3, 7);
        assert_eq!(m2_texture_transform_count(&short[..0x50]), 0);
        assert_eq!(m2_ribbon_emitter_count(&short[..0x50]), 0);
        let mut not_md20 = header_with_counts(3, 7);
        not_md20[0..4].copy_from_slice(b"XXXX");
        assert_eq!(m2_texture_transform_count(&not_md20), 0);
        assert_eq!(m2_ribbon_emitter_count(&not_md20), 0);
    }

    #[test]
    fn empty_model_summary_is_fully_static() {
        let b = header_with_counts(0, 0);
        let summary = parse_m2_animation_summary(&b).expect("an all-zero-array header parses");
        assert_eq!(summary.sequence_count, 0);
        assert!(!summary.seq0_has_bone_motion);
        assert_eq!(summary.seq0_animated_bone_count, 0);
        assert_eq!(summary.seq0_variation_count, 0);
        assert!(summary.global_seq_channels.is_empty());
        assert_eq!(summary.transparency_tracks, (0, 0));
        assert_eq!(summary.color_rgb_tracks, (0, 0));
        assert_eq!(summary.color_alpha_tracks, (0, 0));
        assert_eq!(summary.texture_transform_count, 0);
        assert_eq!(summary.particle_emitter_count, 0);
        assert!(summary.emitter_bones.is_empty());
        assert_eq!(summary.ribbon_emitter_count, 0);
        assert!(summary.is_fully_static());
    }
}
