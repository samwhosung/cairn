use wmo::{ParsedWmo, parse_wmo};

use crate::wmo_mocv::fixed_colors;
use crate::wmo_root::{EXTERIOR_BITS, find_wmo_chunk};
use crate::{ModelBlend, RenderSubmesh, WmoBatchClass, WmoRoot, remap_submesh};

/// [`WmoGroupHeader::group_liquid`] when no liquid fills the whole group.
pub const NO_GROUP_LIQUID: u32 = 0xf;

/// `MOMT` flag: lighting off. The client honours it on exterior groups only.
const MOMT_UNLIT: u32 = 0x01;
const MOMT_TWO_SIDED: u32 = 0x04;
const MOMT_NIGHT_GLOW: u32 = 0x10;
/// `MOMT` flag: a window, which the client's interior draw lights with a light of its own.
const MOMT_WINDOW: u32 = 0x20;

/// A group file's `MOGP` header fields.
#[derive(Debug, Clone, Copy)]
pub struct WmoGroupHeader {
    /// Group flags (`+0x08`): `0x8` is exterior; with neither `0x8` nor `0x40` the group is
    /// interior.
    pub flags: u32,
    /// The first of this group's refs in [`WmoPortals::refs`](crate::WmoPortals::refs) (`+0x24`).
    pub portal_ref_start: u16,
    /// The number of this group's portal refs (`+0x26`).
    pub portal_ref_count: u16,
    /// The group's `WMOGroupID` in `WMOAreaTable` (`+0x38`); `0` when the header is too short.
    pub area_table_id: u32,
    /// Four indices into [`WmoRoot::fogs`] (`+0x30`); `[0; 4]` when the header is too short.
    pub fog_indices: [u8; 4],
    /// The liquid filling the whole group (`+0x34`); [`NO_GROUP_LIQUID`] for none, and when the
    /// header is too short. The client treats all of a group with any other value as under that
    /// liquid, `MLIQ` or not.
    pub group_liquid: u32,
}

/// Reads a group file's `MOGP` header; `None` when the bytes hold no `MOGP` of at least 0x28 bytes.
pub fn wmo_group_header(group_bytes: &[u8]) -> Option<WmoGroupHeader> {
    let mogp = find_wmo_chunk(group_bytes, *b"PGOM")?;
    if mogp.len() < 0x28 {
        return None;
    }
    let u32_at = |i: usize| {
        (mogp.len() >= i + 4)
            .then(|| u32::from_le_bytes([mogp[i], mogp[i + 1], mogp[i + 2], mogp[i + 3]]))
    };
    Some(WmoGroupHeader {
        flags: u32::from_le_bytes([mogp[8], mogp[9], mogp[10], mogp[11]]),
        portal_ref_start: u16::from_le_bytes([mogp[0x24], mogp[0x25]]),
        portal_ref_count: u16::from_le_bytes([mogp[0x26], mogp[0x27]]),
        area_table_id: u32_at(0x38).unwrap_or(0),
        fog_indices: if mogp.len() >= 0x34 {
            [mogp[0x30], mogp[0x31], mogp[0x32], mogp[0x33]]
        } else {
            [0; 4]
        },
        group_liquid: u32_at(0x34).unwrap_or(NO_GROUP_LIQUID),
    })
}

/// An interior group's render faces with their baked vertex colours, in WMO model space: the
/// faces the client's down-ray samples to light an object standing in the group.
#[derive(Clone)]
pub struct FootprintTris {
    pub positions: Vec<[f32; 3]>,
    /// Triangles into [`Self::positions`], in `MOVI` order.
    pub indices: Vec<u16>,
    /// The baked `MOCV` colour per vertex, as RGB bytes.
    pub mocv: Vec<[u8; 3]>,
    /// The `MOPY` flags per triangle. Bit `0x1` on the face hit makes the client light from the
    /// day/night colour instead of the `MOCV` sample.
    pub mopy_flags: Vec<u8>,
    /// The `MOPY` material per triangle, an index into the root's `MOMT` (as in
    /// [`WmoRoot::material_ground_types`]). `0xFF`, or any id past the table, names none.
    pub mopy_material: Vec<u8>,
}

/// `MOPY` flags whose faces the client's down-ray skips: `0x08` marks collision-only faces, and
/// `0x80` is the client's own visited mark, never set on disk.
const FOOTPRINT_REJECT: u8 = 0x88;

/// The footprint faces of a group file; `None` unless it is an interior group with a `MOCV`
/// colour per vertex.
pub fn wmo_group_footprint_tris(group_bytes: &[u8]) -> Option<FootprintTris> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(group_bytes) else {
        return None;
    };
    if group.flags & EXTERIOR_BITS != 0 || group.vertex_colors.len() != group.vertex_positions.len()
    {
        return None;
    }
    let mut indices = Vec::new();
    let mut mopy_flags = Vec::new();
    let mut mopy_material = Vec::new();
    for (ti, tri) in group.vertex_indices.as_chunks::<3>().0.iter().enumerate() {
        let flags = group.material_info.get(ti).map_or(0, |m| m.flags);
        if flags & FOOTPRINT_REJECT != 0 {
            continue;
        }
        indices.extend_from_slice(tri);
        mopy_flags.push(flags);
        mopy_material.push(group.material_info.get(ti).map_or(0xFF, |m| m.material_id));
    }
    Some(FootprintTris {
        positions: group
            .vertex_positions
            .iter()
            .map(|v| [v.x, v.y, v.z])
            .collect(),
        indices,
        mocv: group
            .vertex_colors
            .iter()
            .map(|c| [c.r, c.g, c.b])
            .collect(),
        mopy_flags,
        mopy_material,
    })
}

fn find_mogp_subchunk(group_bytes: &[u8], magic: [u8; 4]) -> Option<&[u8]> {
    let mogp = find_wmo_chunk(group_bytes, *b"PGOM")?;
    let mut off = 0x44usize;
    while off + 8 <= mogp.len() {
        let size = u32::from_le_bytes([mogp[off + 4], mogp[off + 5], mogp[off + 6], mogp[off + 7]])
            as usize;
        let data_start = off + 8;
        let data_end = data_start.saturating_add(size).min(mogp.len());
        if mogp[off..off + 4] == magic {
            return Some(&mogp[data_start..data_end]);
        }
        off = data_end;
    }
    None
}

/// The [`WmoRoot::doodads`] this group places (`MODR`); the client lights each by this group's
/// class and `MOLR` lights. Empty when there are none.
pub fn wmo_group_doodad_refs(group_bytes: &[u8]) -> Vec<u16> {
    find_mogp_subchunk(group_bytes, *b"RDOM")
        .map(|c| {
            c.as_chunks::<2>()
                .0
                .iter()
                .map(|r| u16::from_le_bytes([r[0], r[1]]))
                .collect()
        })
        .unwrap_or_default()
}

/// The root's `MOLT` lights ([`crate::parse_wmo_lights`]) that light this group's doodads
/// (`MOLR`). Empty when there are none, and then the client gives them no point light.
pub fn wmo_group_light_refs(group_bytes: &[u8]) -> Vec<u16> {
    find_mogp_subchunk(group_bytes, *b"RLOM")
        .map(|c| {
            c.as_chunks::<2>()
                .0
                .iter()
                .map(|r| u16::from_le_bytes([r[0], r[1]]))
                .collect()
        })
        .unwrap_or_default()
}

/// One submesh per non-empty render batch of a group file, with textures and materials resolved against
/// `root`; empty when the bytes are not a group. Vertex colours carry the doorway fade
/// ([`wmo_group_fixed_colors`](crate::wmo_group_fixed_colors)).
#[allow(clippy::too_many_lines)]
pub fn wmo_group_submeshes(group_bytes: &[u8], root: &WmoRoot) -> Vec<RenderSubmesh> {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(group_bytes) else {
        return Vec::new();
    };
    let colors = fixed_colors(&group, group_bytes, root).unwrap_or_default();
    let root = &root.parsed;
    let (trans_n, int_n) = find_wmo_chunk(group_bytes, *b"PGOM")
        .filter(|m| m.len() >= 0x2c)
        .map_or((0usize, 0usize), |m| {
            (
                u16::from_le_bytes([m[0x28], m[0x29]]) as usize,
                u16::from_le_bytes([m[0x2a], m[0x2b]]) as usize,
            )
        });
    let mut out = Vec::new();
    {
        let has_normals = group.vertex_normals.len() == group.vertex_positions.len();
        let has_colors = colors.len() == group.vertex_positions.len();
        let interior = (group.flags & EXTERIOR_BITS) == 0;
        let vertex = |g: u32| {
            let p = &group.vertex_positions[g as usize];
            let n = group
                .vertex_normals
                .get(g as usize)
                .map_or([0.0, 1.0, 0.0], |n| [n.x, n.y, n.z]);
            let uv = group
                .texture_coords
                .get(g as usize)
                .map_or([0.0, 0.0], |t| [t.u, t.v]);
            let c = colors.get(g as usize).map_or([1.0, 1.0, 1.0, 1.0], |c| {
                [
                    f32::from(c.r) / 255.0,
                    f32::from(c.g) / 255.0,
                    f32::from(c.b) / 255.0,
                    f32::from(c.a) / 255.0,
                ]
            });
            ([p.x, p.y, p.z], n, uv, c)
        };
        for (bi, batch) in group.render_batches.iter().enumerate() {
            let class = if bi < trans_n {
                WmoBatchClass::Trans
            } else if bi < trans_n + int_n {
                WmoBatchClass::Int
            } else {
                WmoBatchClass::Ext
            };
            let start = batch.start_index as usize;
            let global_indices: Vec<u32> = group
                .vertex_indices
                .get(start..start + batch.count as usize)
                .unwrap_or(&[])
                .iter()
                .map(|&i| u32::from(i))
                .filter(|&g| (g as usize) < group.vertex_positions.len())
                .collect();
            if global_indices.is_empty() {
                continue;
            }
            let material = root.materials.get(batch.material_id as usize);
            let texture = material
                .and_then(|m| m.texture_1_index(&root.texture_offset_index_map))
                .and_then(|i| root.textures.get(i as usize))
                .cloned()
                .filter(|s| !s.is_empty());
            // MOMT blendMode is the client's blend index itself, with no remap as on M2.
            let blend = match material.map(|m| m.blend_mode) {
                Some(0) | None => ModelBlend::Opaque,
                Some(1) => ModelBlend::AlphaTest,
                Some(4) => ModelBlend::Mod,
                Some(5) => ModelBlend::Mod2x,
                Some(_) => ModelBlend::Blend,
            };
            let flags = material.map_or(0, |m| m.flags);
            let emissive = !interior && flags & MOMT_UNLIT != 0;
            let (mut submesh, _globals) = remap_submesh(
                global_indices.into_iter(),
                vertex,
                texture,
                blend,
                flags & MOMT_TWO_SIDED != 0,
                interior,
                emissive,
            );
            submesh.wmo_batch = Some(class);
            submesh.sidn = (flags & MOMT_NIGHT_GLOW != 0)
                .then(|| material.map(|m| m.sidn_rgb))
                .flatten();
            submesh.window = flags & MOMT_WINDOW != 0;
            if !has_normals {
                submesh.normals.clear();
            }
            if !has_colors {
                submesh.vertex_colors.clear();
            }
            // Interior TRANS and INT batches light by the MOCV alpha; everywhere else it is opaque.
            if !(interior && matches!(class, WmoBatchClass::Trans | WmoBatchClass::Int)) {
                for c in &mut submesh.vertex_colors {
                    c[3] = 1.0;
                }
            }
            out.push(submesh);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wmo_root::test_bytes::chunk;

    #[test]
    fn reads_group_header_flags_and_portal_ref_span() {
        let mut hdr = vec![0u8; 68];
        hdr[8..12].copy_from_slice(&0x8u32.to_le_bytes());
        hdr[0x24..0x26].copy_from_slice(&5u16.to_le_bytes());
        hdr[0x26..0x28].copy_from_slice(&3u16.to_le_bytes());
        hdr[0x30..0x34].copy_from_slice(&[2, 0, 0, 0]);
        let mut group = Vec::new();
        group.extend(chunk(*b"REVM", &17u32.to_le_bytes()));
        group.extend(chunk(*b"PGOM", &hdr));

        let h = wmo_group_header(&group).expect("group header");
        assert_eq!(h.flags, 0x8);
        assert_eq!(h.portal_ref_start, 5);
        assert_eq!(h.portal_ref_count, 3);
        assert_eq!(h.fog_indices, [2, 0, 0, 0]);
        assert_eq!(
            h.group_liquid, 0,
            "0x34 is read raw: a zeroed header names liquid type 0, not the 0xf sentinel"
        );
    }

    #[test]
    fn reads_the_whole_group_liquid_override() {
        let group_with = |liquid: u32| {
            let mut hdr = vec![0u8; 68];
            hdr[0x34..0x38].copy_from_slice(&liquid.to_le_bytes());
            let mut group = Vec::new();
            group.extend(chunk(*b"REVM", &17u32.to_le_bytes()));
            group.extend(chunk(*b"PGOM", &hdr));
            group
        };

        let flooded = wmo_group_header(&group_with(0)).expect("group header");
        assert_eq!(flooded.group_liquid, 0);
        assert_ne!(flooded.group_liquid, NO_GROUP_LIQUID);

        assert_eq!(
            wmo_group_header(&group_with(NO_GROUP_LIQUID))
                .expect("group header")
                .group_liquid,
            NO_GROUP_LIQUID
        );

        let mut short = Vec::new();
        short.extend(chunk(*b"REVM", &17u32.to_le_bytes()));
        short.extend(chunk(*b"PGOM", &[0u8; 0x30]));
        assert_eq!(
            wmo_group_header(&short)
                .expect("a short header still parses")
                .group_liquid,
            NO_GROUP_LIQUID
        );
    }
}
