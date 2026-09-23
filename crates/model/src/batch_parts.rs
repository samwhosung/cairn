use std::io::Cursor;

use m2::{M2TextureType, parse_m2};
use wowfile::ByteExt;

use crate::{BoneScaleAnim, CharSkinSlot};

const GLOBAL_SEQUENCES: usize = 0x14;
const BONES: usize = 0x34;
const BONE_SIZE: usize = 0x6c;
const BONE_TRANSLATION: usize = 0x0c;
const BONE_SCALE: usize = 0x44;

pub(crate) fn parent_dir(path: &str) -> &str {
    match path.rfind(['\\', '/']) {
        Some(i) => &path[..i],
        None => "",
    }
}

pub(crate) fn resolve_texture(
    tex: &m2::M2Texture,
    dir: &str,
    skins: &[Option<String>],
) -> (Option<String>, Option<u8>, Option<CharSkinSlot>, bool) {
    let icon_slot = matches!(tex.texture_type, M2TextureType::Other(14));
    let embedded = {
        let f = tex.filename.string.to_string_lossy();
        (!f.is_empty()).then(|| f.into_owned())
    };
    let variation = |i: usize| {
        skins
            .get(i)
            .and_then(Clone::clone)
            .map(|name| format!("{dir}\\{name}.blp"))
    };
    let char_slot = match tex.texture_type {
        M2TextureType::Other(1) => Some(CharSkinSlot::Body),
        M2TextureType::Other(2) => Some(CharSkinSlot::Object),
        M2TextureType::Other(6) => Some(CharSkinSlot::Hair),
        M2TextureType::Other(8) => Some(CharSkinSlot::SkinExtra),
        _ => None,
    };
    match tex.texture_type {
        M2TextureType::Monster1 => (variation(0).or(embedded), Some(0), None, false),
        M2TextureType::Monster2 => (variation(1).or(embedded), Some(1), None, false),
        M2TextureType::Monster3 => (variation(2).or(embedded), Some(2), None, false),
        _ => (embedded, None, char_slot, icon_slot),
    }
}

pub(crate) fn track_constant(track: &m2::M2ScalarTrack) -> Option<f32> {
    if track.keys.is_empty() {
        return Some(1.0);
    }
    track.constant()
}

pub(crate) fn normalize_weights(w: [u8; 4]) -> [f32; 4] {
    let sum = w.iter().map(|&x| f32::from(x)).sum::<f32>();
    if sum <= 0.0 {
        return [1.0, 0.0, 0.0, 0.0];
    }
    [
        f32::from(w[0]) / sum,
        f32::from(w[1]) / sum,
        f32::from(w[2]) / sum,
        f32::from(w[3]) / sum,
    ]
}

pub(crate) fn parse_bone_scale_anim(bytes: &[u8], bone_idx: usize) -> Option<BoneScaleAnim> {
    let bone_count = bytes.u32_at(BONES)? as usize;
    let bones_ofs = bytes.u32_at(BONES + 4)? as usize;
    if bone_idx >= bone_count {
        return None;
    }
    let track = bones_ofs
        .checked_add(bone_idx * BONE_SIZE)?
        .checked_add(BONE_SCALE)?;
    let interp = bytes.u16_at(track)? != 0;
    let gseq = bytes.u16_at(track + 0x02)?;
    if gseq == 0xffff {
        return None;
    }
    let nkeys = bytes.u32_at(track + 0x0c)? as usize;
    let ts_ofs = bytes.u32_at(track + 0x10)? as usize;
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let val_ofs = bytes.u32_at(track + 0x18)? as usize;
    if nkeys <= 1 || nval < nkeys {
        return None;
    }
    let gseq_count = bytes.u32_at(GLOBAL_SEQUENCES)? as usize;
    let gseq_o = bytes.u32_at(GLOBAL_SEQUENCES + 4)? as usize;
    if gseq as usize >= gseq_count {
        return None;
    }
    let duration_ms = bytes.u32_at(gseq_o.checked_add(gseq as usize * 4)?)?;
    if duration_ms == 0 {
        return None;
    }
    let keys = (0..nkeys)
        .map(|k| {
            let t = bytes.u32_at(ts_ofs.checked_add(k * 4)?)?;
            let v = val_ofs.checked_add(k * 12)?;
            Some((
                t,
                [bytes.f32_at(v)?, bytes.f32_at(v + 4)?, bytes.f32_at(v + 8)?],
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(BoneScaleAnim {
        duration_ms,
        interp,
        keys,
    })
}

pub(crate) fn parse_bone_seq_translation(
    bytes: &[u8],
    bone_idx: usize,
    band: (u32, u32),
) -> Option<BoneScaleAnim> {
    let (start, end) = band;
    let duration_ms = end.checked_sub(start).filter(|d| *d > 0)?;
    let bone_count = bytes.u32_at(BONES)? as usize;
    let bones_ofs = bytes.u32_at(BONES + 4)? as usize;
    if bone_idx >= bone_count {
        return None;
    }
    let track = bones_ofs
        .checked_add(bone_idx * BONE_SIZE)?
        .checked_add(BONE_TRANSLATION)?;
    let interp = bytes.u16_at(track)? != 0;
    if bytes.u16_at(track + 0x02)? != 0xffff {
        return None;
    }
    let nkeys = bytes.u32_at(track + 0x0c)? as usize;
    let ts_ofs = bytes.u32_at(track + 0x10)? as usize;
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let val_ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval < nkeys {
        return None;
    }
    let mut keys = Vec::new();
    for k in 0..nkeys {
        let t = bytes.u32_at(ts_ofs.checked_add(k * 4)?)?;
        if t < start || t > end {
            continue;
        }
        let v = val_ofs.checked_add(k * 12)?;
        keys.push((
            t - start,
            [bytes.f32_at(v)?, bytes.f32_at(v + 4)?, bytes.f32_at(v + 8)?],
        ));
    }
    if keys.len() <= 1 {
        return None;
    }
    Some(BoneScaleAnim {
        duration_ms,
        interp,
        keys,
    })
}

/// Indices of the model's billboard bones that are welded to the rest of its geometry, so
/// [`crate::parse_m2_render_submeshes`] never splits them into cards; ascending. Empty when the
/// model or its first skin does not parse.
pub fn non_separable_billboard_bones(bytes: &[u8]) -> Vec<u16> {
    let Ok(format) = parse_m2(&mut Cursor::new(bytes)) else {
        return Vec::new();
    };
    let model = format.model();
    let Ok(skin) = model.parse_embedded_skin(bytes, 0) else {
        return Vec::new();
    };
    let separable = separable_billboard_bones(model, skin.triangles(), skin.indices());
    model
        .bones
        .iter()
        .enumerate()
        .filter(|(i, b)| b.is_billboard() && !separable.get(*i).copied().unwrap_or(false))
        .map(|(i, _)| i as u16)
        .collect()
}

/// The client skins every vertex through the bone palette, a billboard bone's camera-facing matrix
/// among them, so a card welded to other bones bends with them instead of tearing away.
pub(crate) fn separable_billboard_bones(
    model: &m2::M2Model,
    tris: &[u16],
    lookup: &[u16],
) -> Vec<bool> {
    let mut separable = vec![true; model.bones.len()];
    let mut deny = |b: usize| {
        if let Some(s) = separable.get_mut(b) {
            *s = false;
        }
    };
    for v in &model.vertices {
        for i in 0..4 {
            let w = v.bone_weights[i];
            if w != 0 && w != u8::MAX {
                deny(v.bone_indices[i] as usize);
            }
        }
    }
    let sole_bone = |g: usize| -> Option<usize> {
        let v = model.vertices.get(g)?;
        (0..4)
            .find(|&i| v.bone_weights[i] == u8::MAX)
            .map(|i| v.bone_indices[i] as usize)
    };
    for t in tris.as_chunks::<3>().0 {
        let g: Vec<usize> = t
            .iter()
            .filter_map(|&i| lookup.get(i as usize).map(|&x| x as usize))
            .collect();
        let [a, b, c] = g[..] else { continue };
        let (sa, sb, sc) = (sole_bone(a), sole_bone(b), sole_bone(c));
        if sa.is_some() && sa == sb && sb == sc {
            continue;
        }
        for &v in &[a, b, c] {
            let Some(vert) = model.vertices.get(v) else {
                continue;
            };
            for i in 0..4 {
                if vert.bone_weights[i] != 0 {
                    deny(vert.bone_indices[i] as usize);
                }
            }
        }
    }
    separable
}
