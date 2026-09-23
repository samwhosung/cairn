use std::ffi::CString;
use std::io::Cursor;

use wowfile::{ByteExt, capped};

use crate::camera::{parse_camera_lookup, parse_cameras};
use crate::error::{Error, Result};
use crate::model::{
    C2, C3, M2ArrayString, M2Attachment, M2BlendMode, M2Bone, M2BoneFlags, M2Bounds, M2EventMarker,
    M2Format, M2Material, M2Model, M2PlayableAnim, M2RawData, M2RenderFlags, M2Texture,
    M2TextureTransform, M2TextureType, M2Vertex,
};
use crate::track::{Budget, track_fix16, track_quat, track_vec3_timed};

const GLOBAL_SEQUENCES: usize = 0x14;
const ANIMATION_LOOKUP: usize = 0x24;
const PLAYABLE_ANIMATION_LOOKUP: usize = 0x2c;
const BONES: usize = 0x34;
const VERTICES: usize = 0x44;
const VIEWS: usize = 0x4c;
const COLORS: usize = 0x54;
const TEXTURES: usize = 0x5c;
const TRANSPARENCY: usize = 0x64;
const TEXTURE_TRANSFORMS: usize = 0x74;
const RENDER_FLAGS: usize = 0x84;
const TEXTURE_LOOKUP: usize = 0x94;
const TEXTURE_UNIT_LOOKUP: usize = 0x9c;
const TRANSPARENCY_LOOKUP: usize = 0xa4;
const TEXTURE_TRANSFORM_LOOKUP: usize = 0xac;
const BOUNDING_BOX: usize = 0xb4;
const COLLISION_BOX: usize = 0xd0;
const BOUNDING_TRIANGLES: usize = 0xec;
const BOUNDING_VERTICES: usize = 0xf4;
const ATTACHMENTS: usize = 0x104;
const ATTACHMENT_LOOKUP: usize = 0x10c;
const EVENTS: usize = 0x114;
const HEADER_END: usize = EVENTS + 8;

const PIVOT_ATTACHMENT_ID: usize = 17;
const NO_ENTRY: u16 = 0xffff;

#[derive(Clone, Copy)]
struct CountOffset {
    count: usize,
    offset: usize,
}

pub(crate) fn rd_vec3(b: &[u8], o: usize) -> Option<[f32; 3]> {
    Some([b.f32_at(o)?, b.f32_at(o + 4)?, b.f32_at(o + 8)?])
}

pub(crate) fn bytes_from(b: &[u8], offset: usize) -> &[u8] {
    b.get(offset..).unwrap_or_default()
}

/// Reads every `size`-byte record of `a`; one past the end of the file fails the whole array.
fn records<T>(
    b: &[u8],
    a: CountOffset,
    size: usize,
    mut read: impl FnMut(&[u8]) -> Result<T>,
) -> Result<Vec<T>> {
    let mut out = Vec::with_capacity(capped(a.count, size, bytes_from(b, a.offset).len()));
    for i in 0..a.count {
        let rec = b
            .bytes_at(a.offset + i * size, size)
            .ok_or(Error::Truncated)?;
        out.push(read(rec)?);
    }
    Ok(out)
}

fn u16s(b: &[u8], a: CountOffset) -> Result<Vec<u16>> {
    records(b, a, 2, |r| r.u16_at(0).ok_or(Error::Truncated))
}

/// Reads the records of `a` that lie wholly inside the file, for tables whose readers never
/// fail and so cannot stop a corrupt count on their own.
fn whole_records<T>(b: &[u8], a: CountOffset, size: usize, read: impl Fn(usize) -> T) -> Vec<T> {
    let whole = capped(a.count, size, bytes_from(b, a.offset).len());
    (0..whole).map(|i| read(a.offset + i * size)).collect()
}

/// Parses an MD20 model, version 256 to 263, from the start of the cursor's buffer whatever
/// its position. Its colour, transparency and texture tracks decode at most as many bytes as the
/// file holds; past that they read as keyless.
pub fn parse_m2(cursor: &mut Cursor<&[u8]>) -> Result<M2Format> {
    let b: &[u8] = cursor.get_ref();
    if b.len() < 8 || &b[0..4] != b"MD20" {
        return Err(Error::NotMd20);
    }
    let version = b.u32_at(4).ok_or(Error::Truncated)?;
    if !(256..=263).contains(&version) {
        return Err(Error::UnsupportedVersion(version));
    }
    if b.len() < HEADER_END {
        return Err(Error::Truncated);
    }
    let array = |p: usize| CountOffset {
        count: b.u32_at(p).unwrap_or_default() as usize,
        offset: b.u32_at(p + 4).unwrap_or_default() as usize,
    };

    let views = array(VIEWS);
    let vertices = read_vertices(b, array(VERTICES))?;
    let textures = read_textures(b, array(TEXTURES))?;
    let materials = records(b, array(RENDER_FLAGS), 4, |m| {
        Ok(M2Material {
            flags: M2RenderFlags(m.u16_at(0).ok_or(Error::Truncated)?),
            blend_mode: M2BlendMode(m.u16_at(2).ok_or(Error::Truncated)?),
        })
    })?;
    let bones = read_bones(b, array(BONES))?;
    let (attachments, attach_lookup) =
        read_attachments(b, array(ATTACHMENTS), array(ATTACHMENT_LOOKUP), bones.len())?;
    let event_markers = read_events(b, array(EVENTS), bones.len())?;
    let animation_lookup = u16s(b, array(ANIMATION_LOOKUP))?;
    let playable_animation_lookup = records(b, array(PLAYABLE_ANIMATION_LOOKUP), 4, |r| {
        let row = r.u32_at(0).ok_or(Error::Truncated)?;
        Ok(M2PlayableAnim {
            resolved_id: (row & 0xffff) as u16,
            dir_flags: (row >> 16) as u16,
        })
    })?;
    let texture_lookup_table = u16s(b, array(TEXTURE_LOOKUP))?;

    let budget = Budget::of(b);
    let colors = array(COLORS);
    let color_alpha_tracks = whole_records(b, colors, 0x38, |o| track_fix16(b, o + 0x1c, &budget));
    let color_rgb_tracks = whole_records(b, colors, 0x38, |o| track_vec3_timed(b, o, &budget));
    let transparency_tracks =
        whole_records(b, array(TRANSPARENCY), 0x1c, |o| track_fix16(b, o, &budget));
    let texture_unit_lookup = u16s(b, array(TEXTURE_UNIT_LOOKUP))?;
    let transparency_lookup = u16s(b, array(TRANSPARENCY_LOOKUP))?;
    let texture_transforms =
        whole_records(b, array(TEXTURE_TRANSFORMS), 0x54, |o| M2TextureTransform {
            translation: track_vec3_timed(b, o, &budget),
            rotation: track_quat(b, o + 0x1c, &budget),
            scaling: track_vec3_timed(b, o + 0x38, &budget),
        });
    let texture_transform_lookup = u16s(b, array(TEXTURE_TRANSFORM_LOOKUP))?;
    let global_sequences = whole_records(b, array(GLOBAL_SEQUENCES), 4, |o| {
        b.u32_at(o).unwrap_or_default()
    });

    let hull = |a: CountOffset, size: usize| {
        b.bytes_at(a.offset, a.count * size)
            .map(<[u8]>::to_vec)
            .ok_or(Error::Truncated)
    };
    let bounding_triangles = hull(array(BOUNDING_TRIANGLES), 2)?;
    let bounding_vertices = hull(array(BOUNDING_VERTICES), 12)?;

    Ok(M2Format {
        model: M2Model {
            vertices,
            textures,
            materials,
            color_alpha_tracks,
            color_rgb_tracks,
            transparency_tracks,
            transparency_lookup,
            texture_unit_lookup,
            texture_transforms,
            texture_transform_lookup,
            global_sequences,
            bones,
            cameras: parse_cameras(b),
            camera_lookup: parse_camera_lookup(b),
            raw_data: M2RawData {
                texture_lookup_table,
                bounding_triangles,
                bounding_vertices,
            },
            bounds: read_bounds(b),
            pivot_attach_z: attachment_z(
                b,
                PIVOT_ATTACHMENT_ID,
                array(ATTACHMENTS),
                array(ATTACHMENT_LOOKUP),
            ),
            attachments,
            attach_lookup,
            event_markers,
            animation_lookup,
            playable_animation_lookup,
            views: (views.count as u32, views.offset as u32),
            version,
        },
    })
}

fn read_bounds(b: &[u8]) -> M2Bounds {
    let f32_at = |p: usize| b.f32_at(p).unwrap_or_default();
    let vec3_at = |p: usize| rd_vec3(b, p).unwrap_or_default();
    M2Bounds {
        bounding_box_min: vec3_at(BOUNDING_BOX),
        bounding_box_max: vec3_at(BOUNDING_BOX + 12),
        bounding_sphere_radius: f32_at(BOUNDING_BOX + 24),
        collision_box_min: vec3_at(COLLISION_BOX),
        collision_box_max: vec3_at(COLLISION_BOX + 12),
        collision_sphere_radius: f32_at(COLLISION_BOX + 24),
    }
}

fn read_vertices(b: &[u8], a: CountOffset) -> Result<Vec<M2Vertex>> {
    records(b, a, 48, |v| {
        Ok(M2Vertex {
            position: c3_in(v, 0)?,
            bone_weights: [v[12], v[13], v[14], v[15]],
            bone_indices: [v[16], v[17], v[18], v[19]],
            normal: c3_in(v, 20)?,
            tex_coords: C2 {
                x: v.f32_at(32).ok_or(Error::Truncated)?,
                y: v.f32_at(36).ok_or(Error::Truncated)?,
            },
        })
    })
}

fn read_textures(b: &[u8], a: CountOffset) -> Result<Vec<M2Texture>> {
    records(b, a, 16, |t| {
        let at = |o: usize| t.u32_at(o).ok_or(Error::Truncated);
        let flags = at(4)?;
        let raw = b
            .bytes_at(at(12)? as usize, at(8)? as usize)
            .unwrap_or_default();
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        Ok(M2Texture {
            texture_type: M2TextureType::from_u32(at(0)?),
            wrap_x: flags & 0x1 != 0,
            wrap_y: flags & 0x2 != 0,
            filename: M2ArrayString {
                string: CString::new(&raw[..end]).unwrap_or_default(),
            },
        })
    })
}

fn read_bones(b: &[u8], a: CountOffset) -> Result<Vec<M2Bone>> {
    records(b, a, 108, |r| {
        let mut pivot = c3_in(r, 96)?;
        for c in [&mut pivot.x, &mut pivot.y, &mut pivot.z] {
            if c.is_nan() {
                *c = 0.0;
            }
        }
        Ok(M2Bone {
            key_bone: r.u32_at(0).ok_or(Error::Truncated)? as i32 as i16,
            flags: M2BoneFlags(r.u32_at(4).ok_or(Error::Truncated)?),
            parent: r.u16_at(8).ok_or(Error::Truncated)? as i16,
            pivot,
        })
    })
}

fn bone_index(bone: u32, n_bones: usize) -> Option<u16> {
    u16::try_from(bone).ok().filter(|&i| (i as usize) < n_bones)
}

/// The attachments kept, and the lookup translated to their indices.
fn read_attachments(
    b: &[u8],
    a: CountOffset,
    lookup: CountOffset,
    n_bones: usize,
) -> Result<(Vec<M2Attachment>, Vec<u16>)> {
    let in_file = records(b, a, 48, |r| {
        Ok((
            r.u32_at(0).ok_or(Error::Truncated)?,
            r.u32_at(4).ok_or(Error::Truncated)?,
            vec3_in(r, 8)?,
        ))
    })?;
    let mut kept = Vec::with_capacity(in_file.len());
    let mut kept_index = Vec::with_capacity(in_file.len());
    for (id, bone, position) in in_file {
        let fits = u16::try_from(id).ok().zip(bone_index(bone, n_bones));
        kept_index.push(match fits {
            Some((id, bone)) => {
                kept.push(M2Attachment { id, bone, position });
                u16::try_from(kept.len() - 1).unwrap_or(NO_ENTRY)
            }
            None => NO_ENTRY,
        });
    }
    let lookup = u16s(b, lookup)?
        .into_iter()
        .map(|i| kept_index.get(i as usize).copied().unwrap_or(NO_ENTRY))
        .collect();
    Ok((kept, lookup))
}

fn read_events(b: &[u8], a: CountOffset, n_bones: usize) -> Result<Vec<M2EventMarker>> {
    let markers = records(b, a, 44, |e| {
        let bone = e.u32_at(8).ok_or(Error::Truncated)?;
        let position = vec3_in(e, 12)?;
        Ok(bone_index(bone, n_bones).map(|bone| M2EventMarker {
            ident: [e[0], e[1], e[2], e[3]],
            bone,
            position,
        }))
    })?;
    Ok(markers.into_iter().flatten().collect())
}

fn vec3_in(r: &[u8], o: usize) -> Result<[f32; 3]> {
    rd_vec3(r, o).ok_or(Error::Truncated)
}

fn c3_in(r: &[u8], o: usize) -> Result<C3> {
    let [x, y, z] = vec3_in(r, o)?;
    Ok(C3 { x, y, z })
}

/// The Z of the file's attachment record that `id` resolves to through the lookup, before any
/// record is dropped for an out-of-range bone.
fn attachment_z(b: &[u8], id: usize, attachments: CountOffset, lookup: CountOffset) -> Option<f32> {
    if id >= lookup.count {
        return None;
    }
    let idx = b.u16_at(lookup.offset + id * 2)? as usize;
    if idx >= attachments.count {
        return None;
    }
    b.f32_at(attachments.offset + idx * 48 + 16)
}
