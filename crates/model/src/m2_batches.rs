use std::io::Cursor;

use m2::parse_m2;
use mpq::Chain;

use crate::batch_parts::{
    normalize_weights, parent_dir, parse_bone_scale_anim, parse_bone_seq_translation,
    resolve_texture, separable_billboard_bones, track_constant,
};
use crate::key_anim::SeqSlot;
use crate::{
    AlphaAnim, Billboard, BillboardKind, BoneSpin, Error, FogPolicy, ModelBlend, RenderSubmesh,
    le_u16, le_u32, m2_bone_spins, mat_anim, model_path, remap_submesh, tex_anim,
};

const SEQUENCES: usize = 0x1c;
const SEQUENCE_SIZE: usize = 0x44;

/// Reads the model at `raw_path` (`.mdx` and `.mdl` read as `.m2`) and parses it with
/// [`parse_m2_render_submeshes`], without creature skins.
pub fn load_m2_mesh(chain: &Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>, Error> {
    load_m2_mesh_skinned(chain, raw_path, &[])
}

/// [`load_m2_mesh`], filling the `Monster1/2/3` texture slots from `skins`: a creature display's
/// texture variation names, in order.
pub fn load_m2_mesh_skinned(
    chain: &Chain,
    raw_path: &str,
    skins: &[Option<String>],
) -> Result<Vec<RenderSubmesh>, Error> {
    let path = model_path(raw_path);
    let bytes = chain.read(&path).map_err(Error::Chain)?;
    parse_m2_render_submeshes(&bytes, parent_dir(&path), skins)
}

/// [`m2_bone_spins`] of the model at `raw_path`, read as [`load_m2_mesh`] reads it.
pub fn load_m2_bone_spins(
    chain: &Chain,
    raw_path: &str,
) -> Result<std::collections::HashMap<u16, BoneSpin>, Error> {
    let bytes = chain.read(&model_path(raw_path)).map_err(Error::Chain)?;
    Ok(m2_bone_spins(&bytes))
}

/// Parses an M2 file into render submeshes in skin-batch order: one per batch, or for a batch
/// holding rigid billboard cards, one per card bone and one for any other triangles.
/// `Monster1/2/3` slots read `<dir>\<name>.blp` from `skins`. Empty batches and batches the client
/// never draws are left out, and a model boxed flat on one axis whose batches are all opaque
/// `WHITE1.BLP` yields none. Fails when the model or its first embedded skin does not parse.
#[allow(clippy::too_many_lines)]
pub fn parse_m2_render_submeshes(
    bytes: &[u8],
    dir: &str,
    skins: &[Option<String>],
) -> Result<Vec<RenderSubmesh>, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    let model = format.model();
    let skin = model.parse_embedded_skin(bytes, 0).map_err(Error::Skin)?;

    let vertex = |g: u32| {
        let v = &model.vertices[g as usize];
        (
            [v.position.x, v.position.y, v.position.z],
            [v.normal.x, v.normal.y, v.normal.z],
            [v.tex_coords.x, v.tex_coords.y],
            [1.0, 1.0, 1.0, 1.0],
        )
    };
    let skin_vertices = skin.indices();
    let skin_triangles = skin.triangles();
    let global = |t_range: std::ops::Range<usize>| -> Vec<u32> {
        skin_triangles
            .get(t_range)
            .unwrap_or(&[])
            .iter()
            .filter_map(|&t| skin_vertices.get(t as usize).copied())
            .map(u32::from)
            .filter(|&g| (g as usize) < model.vertices.len())
            .collect()
    };
    let separable = separable_billboard_bones(model, skin_triangles, skin_vertices);

    let seq_bands: Vec<(u16, (u32, u32), bool)> = {
        let (n, o) = (
            le_u32(bytes, SEQUENCES) as usize,
            le_u32(bytes, SEQUENCES + 4) as usize,
        );
        (0..n)
            .map_while(|i| {
                let e = o + i * SEQUENCE_SIZE;
                (e + SEQUENCE_SIZE <= bytes.len()).then(|| {
                    (
                        le_u16(bytes, e),
                        (le_u32(bytes, e + 0x04), le_u32(bytes, e + 0x08)),
                        le_u32(bytes, e + 0x10) & 1 == 0,
                    )
                })
            })
            .collect()
    };
    let seq_slots: Vec<SeqSlot> = seq_bands
        .iter()
        .enumerate()
        .map(|(file_index, &(_, band_ms, looping))| SeqSlot {
            file_index,
            band_ms,
            looping,
        })
        .collect();
    let seq0_slot = seq_slots.first().copied();

    let sections = skin.submeshes();
    let mut out = Vec::new();
    for batch in skin.batches() {
        let Some(section) = sections.get(batch.skin_section_index as usize) else {
            continue;
        };
        let start = section.triangle_start as usize;
        let global_indices = global(start..start + section.triangle_count as usize);
        if global_indices.is_empty() {
            continue;
        }
        let tex_record = model
            .raw_data
            .texture_lookup_table
            .get(batch.texture_combo_index as usize)
            .and_then(|&ti| model.textures.get(ti as usize));
        let (wrap_x, wrap_y) = tex_record.map_or((true, true), |t| (t.wrap_x, t.wrap_y));
        let (texture, skin_slot, char_slot, icon_slot) = tex_record
            .map_or((None, None, None, false), |t| {
                resolve_texture(t, dir, skins)
            });
        let material = model.materials.get(batch.material_index as usize);
        let blend = match material.map(|m| m.blend_mode.bits()) {
            Some(0) | None => ModelBlend::Opaque,
            Some(1) => ModelBlend::AlphaTest,
            Some(5) => ModelBlend::Mod,
            Some(6) => ModelBlend::Mod2x,
            Some(_) => ModelBlend::Blend,
        };
        let additive = matches!(material.map(|m| m.blend_mode.bits()), Some(3 | 4));
        let two_sided = material.is_some_and(|m| m.flags.bits() & 0x04 != 0);
        let env_map = model.stage_is_env_mapped(batch, 0);
        let unlit = material.is_some_and(|m| m.flags.bits() & 0x01 != 0);
        let no_depth_write = material.is_some_and(|m| m.flags.bits() & 0x10 != 0);
        let no_depth_test = material.is_some_and(|m| m.flags.bits() & 0x08 != 0);
        let fog_policy = match material {
            Some(m) if m.flags.bits() & 0x02 != 0 => FogPolicy::Off,
            Some(m) => match m.blend_mode.bits() {
                3 | 4 => FogPolicy::Black,
                5 => FogPolicy::White,
                6 => FogPolicy::Grey,
                _ => FogPolicy::Scene,
            },
            None => FogPolicy::Scene,
        };
        // The client multiplies instance alpha, colour alpha and transparency weight, and skips
        // the batch when the product is zero or below, whatever its blend mode.
        let color_alpha = if (batch.color_index as usize) < model.color_alpha_tracks.len() {
            track_constant(&model.color_alpha_tracks[batch.color_index as usize])
        } else {
            Some(1.0)
        };
        let weight = if batch.texture_count != 0 {
            model
                .transparency_lookup
                .get(batch.weight_combo_index as usize)
                .and_then(|&t| model.transparency_tracks.get(t as usize))
                .map_or(Some(1.0), track_constant)
        } else {
            Some(1.0) // without textures the weight does not apply
        };
        if let (Some(c), Some(w)) = (color_alpha, weight)
            && c * w <= 0.0
        {
            continue;
        }
        // The client reads each track through the playing sequence's own key window, so both
        // factors bake per sequence slot.
        let alpha_anim = {
            let color_track = model.color_alpha_tracks.get(batch.color_index as usize);
            let weight_track = (batch.texture_count != 0)
                .then(|| {
                    model
                        .transparency_lookup
                        .get(batch.weight_combo_index as usize)
                        .and_then(|&t| model.transparency_tracks.get(t as usize))
                })
                .flatten();
            let per_seq = seq_slots
                .iter()
                .map(|&slot| mat_anim::AlphaSeq {
                    color: color_track.and_then(|t| {
                        mat_anim::bake_scalar_anim(t, &model.global_sequences, Some(slot))
                    }),
                    weight: weight_track.and_then(|t| {
                        mat_anim::bake_scalar_anim(t, &model.global_sequences, Some(slot))
                    }),
                })
                .collect();
            AlphaAnim::new(per_seq)
        };
        let uv_anim = tex_anim::bake_uv_anim(model, batch.texture_transform_combo_index, seq0_slot);
        let uv_seq = tex_anim::bake_uv_seqs(model, batch.texture_transform_combo_index, &seq_slots)
            .filter(|set| set.uniform().is_none());
        let uv_rot_seq =
            tex_anim::bake_uv_rot_seqs(model, batch.texture_transform_combo_index, &seq_slots);
        let uv_scale_seq =
            tex_anim::bake_uv_scale_seqs(model, batch.texture_transform_combo_index, &seq_slots);
        let rgb_track = model.color_rgb_tracks.get(batch.color_index as usize);
        let rgb_anim =
            rgb_track.and_then(|t| mat_anim::bake_rgb_anim(t, &model.global_sequences, seq0_slot));
        let rgb_seq = rgb_track
            .and_then(|t| mat_anim::bake_rgb_seqs(t, &model.global_sequences, &seq_slots))
            .filter(|set| set.uniform().is_none());
        let make_billboard = |bone_idx: usize| -> Option<Billboard> {
            let bone = model.bones.get(bone_idx)?;
            Some(Billboard {
                pivot: [bone.pivot.x, bone.pivot.y, bone.pivot.z],
                bone: bone_idx as u16,
                kind: BillboardKind::from_bone_flags(bone.flags.bits())?,
                scale_anim: parse_bone_scale_anim(bytes, bone_idx),
                seq_translations: seq_bands
                    .iter()
                    .filter_map(|&(id, band, _)| {
                        Some((id, parse_bone_seq_translation(bytes, bone_idx, band)?))
                    })
                    .collect(),
            })
        };
        let primary_billboard_bone = |g: u32| -> Option<usize> {
            let b = model.vertices.get(g as usize)?.bone_indices[0] as usize;
            (model.bones.get(b).is_some_and(m2::M2Bone::is_billboard)
                && separable.get(b).copied().unwrap_or(false))
            .then_some(b)
        };
        let mut groups: Vec<(Option<usize>, Vec<u32>)> = Vec::new();
        for tri in global_indices.as_chunks::<3>().0 {
            let key = primary_billboard_bone(tri[0]);
            if let Some(pos) = groups.iter().position(|(k, _)| *k == key) {
                groups[pos].1.extend_from_slice(tri);
            } else {
                groups.push((key, tri.to_vec()));
            }
        }
        let color_tint: Option<[f32; 4]> = match &rgb_anim {
            Some(_) => None,
            // The client applies no M2Color to a Mod or Mod2x batch.
            None if matches!(blend, ModelBlend::Mod | ModelBlend::Mod2x) => None,
            None => model
                .color_rgb_tracks
                .get(batch.color_index as usize)
                .and_then(|t| t.keys.first())
                .map(|&(_, rgb)| [rgb[0], rgb[1], rgb[2], 1.0]),
        };
        let interior = false;
        for (bone, idx) in groups {
            let (mut sub, globals) = remap_submesh(
                idx.into_iter(),
                vertex,
                texture.clone(),
                blend,
                two_sided,
                interior,
                unlit,
            );
            sub.billboard = bone.and_then(make_billboard);
            sub.welded_billboard = globals.iter().any(|&g| {
                let v = &model.vertices[g as usize];
                (0..4).any(|i| {
                    let b = v.bone_indices[i] as usize;
                    v.bone_weights[i] != 0
                        && model.bones.get(b).is_some_and(m2::M2Bone::is_billboard)
                        && !separable.get(b).copied().unwrap_or(false)
                })
            });
            sub.env_map = env_map;
            sub.additive = additive;
            sub.no_depth_write = no_depth_write;
            sub.no_depth_test = no_depth_test;
            sub.fog_policy = fog_policy;
            sub.skin_slot = skin_slot;
            sub.geoset_id = section.id;
            sub.section = Some(batch.skin_section_index);
            sub.wrap_x = wrap_x;
            sub.wrap_y = wrap_y;
            sub.char_slot = char_slot;
            sub.icon_slot = icon_slot;
            sub.joints = globals
                .iter()
                .map(|&g| {
                    let bi = model.vertices[g as usize].bone_indices;
                    [
                        u16::from(bi[0]),
                        u16::from(bi[1]),
                        u16::from(bi[2]),
                        u16::from(bi[3]),
                    ]
                })
                .collect();
            sub.weights = globals
                .iter()
                .map(|&g| normalize_weights(model.vertices[g as usize].bone_weights))
                .collect();

            match color_tint {
                Some(c) => sub.vertex_colors = vec![c; sub.positions.len()],
                None => sub.vertex_colors.clear(),
            }
            sub.alpha_anim.clone_from(&alpha_anim);
            sub.uv_anim.clone_from(&uv_anim);
            sub.uv_seq.clone_from(&uv_seq);
            sub.uv_rot_seq.clone_from(&uv_rot_seq);
            sub.uv_scale_seq.clone_from(&uv_scale_seq);
            sub.rgb_anim.clone_from(&rgb_anim);
            sub.rgb_seq.clone_from(&rgb_seq);
            out.push(sub);
        }
    }
    if is_white1_placeholder(
        model.bounds.bounding_box_min,
        model.bounds.bounding_box_max,
        &out,
    ) {
        out.clear();
    }
    Ok(out)
}

fn is_white1_placeholder(bbox_min: [f32; 3], bbox_max: [f32; 3], subs: &[RenderSubmesh]) -> bool {
    if subs.is_empty() {
        return false;
    }
    let degenerate = (0..3).any(|k| (bbox_max[k] - bbox_min[k]).abs() < 1e-3);
    degenerate
        && subs.iter().all(|s| {
            matches!(s.blend, ModelBlend::Opaque)
                && !s.additive
                && s.texture
                    .as_deref()
                    .is_some_and(|t| t.to_ascii_lowercase().ends_with("white1.blp"))
        })
}
