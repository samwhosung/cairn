use std::collections::HashMap;
use std::io::Cursor;

use m2::parse_m2;

use crate::{
    BillboardKind, BoneSpin, Error, ParentArm, le_f32, le_u16, le_u32, parse_m2_animations,
};

/// One bone of a model's rest skeleton, in raw WoW model space. Vanilla M2 has no inverse bind
/// matrices: a bone at rest is the identity, and its animation turns and scales about `pivot`.
#[derive(Debug, Clone, Copy)]
pub struct SkeletonBone {
    /// The parent bone's index, `-1` for a root.
    pub parent: i16,
    pub pivot: [f32; 3],
    /// The key-bone id, `-1` for none.
    pub key_bone: i16,
    /// The camera-facing mode of bone flags `0x08`..`0x40`, `None` for an ordinary bone.
    /// Descendants inherit it: see [`Skeleton::billboard_host`].
    pub billboard: Option<BillboardKind>,
    /// How bone flags `0x1`/`0x2`/`0x4` rebuild the parent matrix from the model's root matrix,
    /// `None` for an ordinary bone.
    pub parent_arm: Option<ParentArm>,
}

/// A model's bones in file order, the indices
/// [`RenderSubmesh::joints`](crate::RenderSubmesh::joints) use. Empty for a boneless model.
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    pub bones: Vec<SkeletonBone>,
}

impl Skeleton {
    /// The nearest bone at or above `bone` in its parent chain with a billboard mode, or `None`
    /// when the chain ends without one. Descendants inherit a billboard, so this bone decides how
    /// `bone` faces the camera.
    pub fn billboard_host(&self, bone: u16) -> Option<usize> {
        let mut i = usize::from(bone);
        for _ in 0..=self.bones.len() {
            let b = self.bones.get(i)?;
            if b.billboard.is_some() {
                return Some(i);
            }
            i = usize::try_from(b.parent).ok()?;
        }
        None
    }
}

/// Parse the M2 bone hierarchy into a [`Skeleton`].
pub fn parse_m2_skeleton(bytes: &[u8]) -> Result<Skeleton, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    let bones = format
        .model()
        .bones
        .iter()
        .map(|b| SkeletonBone {
            parent: b.parent,
            pivot: [b.pivot.x, b.pivot.y, b.pivot.z],
            key_bone: b.key_bone,
            billboard: BillboardKind::from_bone_flags(b.flags.bits()),
            parent_arm: ParentArm::from_bone_flags(b.flags.bits()),
        })
        .collect();
    Ok(Skeleton { bones })
}

/// One attachment point: the attachment id it answers to, the bone it rides, and its position in
/// raw WoW model space.
#[derive(Debug, Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    pub position: [f32; 3],
}

/// The attachment points the model's attachment lookup resolves, one per attachment id, in id
/// order; empty for a model without any.
///
/// Records the lookup does not name are left out, so where several records share an id only the
/// looked-up one is returned. [`M2Attachment::id`] is the lookup index, the id the client
/// resolves; the record's own id field plays no part.
pub fn parse_m2_attachments(bytes: &[u8]) -> Result<Vec<M2Attachment>, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    let model = format.model();
    Ok((0..model.attach_lookup.len())
        .filter_map(|id| {
            let a = model.attachment(id as u16)?;
            Some(M2Attachment {
                id: id as u16,
                bone: a.bone,
                position: a.position,
            })
        })
        .collect())
}

/// One event record read as a positional marker: its 4CC, the bone it rides, and its position in
/// raw WoW model space. The fire times are in
/// [`ModelAnimation::events`](crate::ModelAnimation::events).
#[derive(Debug, Clone, Copy)]
pub struct EventMarker {
    /// The identifier as it reads, `*b"$CSL"`.
    pub ident: [u8; 4],
    pub bone: u16,
    pub position: [f32; 3],
}

/// The model's event markers in file order; empty for a model with no events.
pub fn parse_m2_event_markers(bytes: &[u8]) -> Result<Vec<EventMarker>, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    Ok(format
        .model()
        .event_markers
        .iter()
        .map(|m| EventMarker {
            ident: m.ident,
            bone: m.bone,
            position: m.position,
        })
        .collect())
}

/// A bow's string ends, the `$WTT` (top) and `$WTB` (bottom) event markers, each as
/// `(bone index, position in raw WoW model space)`.
#[derive(Clone, Copy)]
pub struct StringAnchors {
    pub top: (u16, [f32; 3]),
    pub bottom: (u16, [f32; 3]),
}

/// Read the `$WTT`/`$WTB` string anchors from the raw event table; `None` unless both are there.
///
/// Panics when `b` is shorter than `0x11c` bytes.
pub fn parse_m2_string_anchors(b: &[u8]) -> Option<StringAnchors> {
    let (ev_count, ev_ofs) = (le_u32(b, 0x114) as usize, le_u32(b, 0x118) as usize);
    let (mut top, mut bottom) = (None, None);
    for e in 0..ev_count {
        let erec = ev_ofs + e * 44;
        if erec + 44 > b.len() {
            break;
        }
        let ident = b.get(erec..erec + 4)?;
        let anchor = (
            le_u32(b, erec + 8) as u16,
            [
                le_f32(b, erec + 12),
                le_f32(b, erec + 16),
                le_f32(b, erec + 20),
            ],
        );
        match ident {
            b"$WTT" => top = Some(anchor),
            b"$WTB" => bottom = Some(anchor),
            _ => {}
        }
    }
    Some(StringAnchors {
        top: top?,
        bottom: bottom?,
    })
}

/// The first `$CCH` event marker, the fishing line's anchor on the pole, as `(bone index,
/// position in raw WoW model space)`; `None` for a model without one.
///
/// Panics when `b` is shorter than `0x11c` bytes.
pub fn parse_m2_cch_marker(b: &[u8]) -> Option<(u16, [f32; 3])> {
    let (ev_count, ev_ofs) = (le_u32(b, 0x114) as usize, le_u32(b, 0x118) as usize);
    for e in 0..ev_count {
        let erec = ev_ofs + e * 44;
        if erec + 44 > b.len() {
            break;
        }
        if b.get(erec..erec + 4)? == b"$CCH" {
            return Some((
                le_u32(b, erec + 8) as u16,
                [
                    le_f32(b, erec + 12),
                    le_f32(b, erec + 16),
                    le_f32(b, erec + 20),
                ],
            ));
        }
    }
    None
}

/// One row of the playable-animation lookup, indexed by the requested `AnimationData.dbc` id.
#[derive(Debug, Clone, Copy)]
pub struct PlayableAnim {
    /// The `AnimationData.dbc` id the model plays for the requested one.
    pub resolved_id: u16,
    /// The row's direction or variant code.
    pub dir_flags: u16,
}

/// Parse the M2's playable-animation lookup: for each requested `AnimationData.dbc` id, the id the
/// model plays instead. Empty for a model without the table.
pub fn parse_m2_playable_animation_lookup(bytes: &[u8]) -> Result<Vec<PlayableAnim>, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    Ok(format
        .model()
        .playable_animation_lookup
        .iter()
        .map(|p| PlayableAnim {
            resolved_id: p.resolved_id,
            dir_flags: p.dir_flags,
        })
        .collect())
}

/// Parse the M2's animation lookup: for each `AnimationData.dbc` id, the index of the model's first
/// sequence with that id, `0xffff` for none.
pub fn parse_m2_animation_lookup(bytes: &[u8]) -> Result<Vec<u16>, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    Ok(format.model().animation_lookup.clone())
}

/// The bones that spin rigidly in the model's `anim_id` 0 sequence, keyed by bone index (see
/// [`BoneSpin`]); empty when none does.
///
/// A bone qualifies when it is a root and its only keys in that sequence are two or more
/// rotations. The sequence is picked by `anim_id`, never by position: id 0 is the one the client
/// arms when it loads a model that has it.
pub fn m2_bone_spins(bytes: &[u8]) -> HashMap<u16, BoneSpin> {
    let mut out = HashMap::new();
    let Ok(skeleton) = parse_m2_skeleton(bytes) else {
        return out;
    };
    let anims = parse_m2_animations(bytes);
    let Some(seq) = anims.iter().find(|a| a.anim_id == 0) else {
        return out;
    };
    let (bone_count, bone_ofs) = (le_u32(bytes, 0x34) as usize, le_u32(bytes, 0x38) as usize);
    for keys in &seq.bones {
        let idx = keys.bone as usize;
        if keys.rotation.len() < 2 || !keys.translation.is_empty() || !keys.scale.is_empty() {
            continue;
        }
        let Some(bone) = skeleton.bones.get(idx).filter(|b| b.parent < 0) else {
            continue;
        };
        let interp = idx < bone_count
            && bone_ofs
                .checked_add(idx * 0x6c + 0x28)
                .is_some_and(|t| t + 2 <= bytes.len() && le_u16(bytes, t) != 0);
        out.insert(
            keys.bone,
            BoneSpin {
                pivot: bone.pivot,
                duration: seq.duration,
                interp,
                keys: keys.rotation.clone(),
            },
        );
    }
    out
}
