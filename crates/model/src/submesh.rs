use crate::{AlphaAnim, BillboardKind, BoneScaleAnim, RgbAnim, SeqLoops, UvAnim};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelBlend {
    Opaque,
    AlphaTest,
    /// Alpha-blended or additive.
    Blend,
    /// Multiplies what is drawn: `out = src · dst`.
    Mod,
    /// `out = 2 · src · dst`, neutral at mid-grey.
    Mod2x,
}

/// A texture the client binds at runtime by the M2 texture record's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharSkinSlot {
    Body,
    Hair,
    /// The item's own texture on a weapon or shield, the cape on a character.
    Object,
    /// The extra skin texture, chosen by skin colour and not composited with the body.
    SkinExtra,
}

/// The M2 billboard bone a batch rides, which the client turns to face the camera about `pivot`
/// (model space) every frame.
#[derive(Debug, Clone)]
pub struct Billboard {
    pub pivot: [f32; 3],
    pub bone: u16,
    pub kind: BillboardKind,
    pub scale_anim: Option<BoneScaleAnim>,
    /// `(animation id, loop)` for each sequence with more than one translation key on the bone,
    /// keys rebased to the sequence start.
    pub seq_translations: Vec<(u16, BoneScaleAnim)>,
}

/// The section of its WMO group a batch sits in: a group lists its TRANS batches first, then
/// INT, then EXT. In an interior group TRANS blends lit and unlit by the `MOCV` alpha, INT is
/// shaded by `MOCV` alone with its alpha as a glow mask, and EXT is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WmoBatchClass {
    Trans,
    Int,
    Ext,
}

/// The colour an M2 batch fogs toward: additive blends to black, `Mod` to white, `Mod2x` to grey,
/// render flag `0x02` off, the rest to the scene's. WMO batches are always `Scene`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FogPolicy {
    #[default]
    Scene = 0,
    Black = 1,
    White = 2,
    Grey = 3,
    Off = 4,
}

/// One render batch, standalone geometry with its material, in model space (WoW axes, Z up).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct RenderSubmesh {
    pub positions: Vec<[f32; 3]>,
    /// Empty when the source authors no normals.
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub texture: Option<String>,
    /// The creature skin this batch takes from its display record: `Some(0..=2)` for `Monster1..3`.
    pub skin_slot: Option<u8>,
    /// The skin section's id, which character geosets are chosen by; `0` for WMO.
    pub geoset_id: u16,
    pub char_slot: Option<CharSkinSlot>,
    pub blend: ModelBlend,
    /// Repeat when set, clamp to the edge when clear: M2 texture flag `0x1`, and `0x2` for
    /// `wrap_y`; always set for WMO.
    pub wrap_x: bool,
    pub wrap_y: bool,
    pub two_sided: bool,
    /// Per-vertex indices into the M2 bone table, with `weights` summing to 1; empty for WMO.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Per-vertex RGBA, empty for white: a WMO group's `MOCV` whitened near doorways to exterior
    /// groups, or an M2 batch's static tint. Alpha is 1 except on interior TRANS and INT batches.
    pub vertex_colors: Vec<[f32; 4]>,
    /// In a WMO group whose `MOGP` flags have neither `0x08` nor `0x40` set.
    pub interior: bool,
    /// Unlit: M2 material flag `0x01`, or `MOMT` flag `0x01` outside interior groups.
    pub emissive: bool,
    /// Texture type 14: an icon filled in at runtime.
    pub icon_slot: bool,
    /// The `MOMT` self-illumination colour (flag `0x10`), emitted in proportion to the night.
    pub sidn: Option<[u8; 3]>,
    /// `MOMT` flag `0x20`: in an interior group, lit with diffuse at the midpoint of direct and
    /// ambient light, and ambient at that midpoint plus 16.
    pub window: bool,
    pub additive: bool,
    /// M2 render flag `0x10`. Every other batch writes depth, transparent ones included.
    pub no_depth_write: bool,
    pub no_depth_test: bool,
    pub fog_policy: FogPolicy,
    pub billboard: Option<Billboard>,
    /// Holds geometry on a billboard bone welded to the rest of the mesh: no rigid transform fits.
    pub welded_billboard: bool,
    /// `None` when the colour alpha and the transparency weight are 1 in every sequence.
    pub alpha_anim: Option<AlphaAnim>,
    /// The texture transform's translation loop in sequence slot 0.
    pub uv_anim: Option<UvAnim>,
    /// `uv_anim` per sequence slot, set only when the slots differ.
    pub uv_seq: Option<SeqLoops<[f32; 2]>>,
    pub uv_rot_seq: Option<SeqLoops<[f32; 4]>>,
    pub uv_scale_seq: Option<SeqLoops<[f32; 2]>>,
    /// The tint's loop in sequence slot 0 when it moves there; `vertex_colors` is then empty.
    pub rgb_anim: Option<RgbAnim>,
    /// `rgb_anim` per sequence slot, set only when the slots differ.
    pub rgb_seq: Option<SeqLoops<[f32; 3]>>,
    pub wmo_batch: Option<WmoBatchClass>,
    /// The index of the M2 skin section the triangles come from; `None` for WMO. Submeshes with
    /// the same section and billboard bone draw the same triangles.
    pub section: Option<u16>,
    /// Stage 0 takes a generated environment coordinate in place of `uvs`.
    pub env_map: bool,
}

impl Default for RenderSubmesh {
    fn default() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            texture: None,
            skin_slot: None,
            geoset_id: 0,
            char_slot: None,
            blend: ModelBlend::Opaque,
            wrap_x: true,
            wrap_y: true,
            two_sided: false,
            joints: Vec::new(),
            weights: Vec::new(),
            vertex_colors: Vec::new(),
            interior: false,
            emissive: false,
            icon_slot: false,
            sidn: None,
            window: false,
            additive: false,
            no_depth_write: false,
            no_depth_test: false,
            fog_policy: FogPolicy::Scene,
            billboard: None,
            welded_billboard: false,
            alpha_anim: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            rgb_anim: None,
            rgb_seq: None,
            wmo_batch: None,
            env_map: false,
            section: None,
        }
    }
}

impl RenderSubmesh {
    /// Whether this is a flat billboard card whose normal points to −X. A spherical billboard
    /// turns bone-local +X toward the viewer, and M2 bones have no bind rotation, so such a card
    /// shows its back.
    pub fn billboard_card_faces_away(&self) -> bool {
        self.billboard.is_some() && self.plane_normal().is_some_and(|n| n[0] < -Self::EDGE_ON_X)
    }

    const EDGE_ON_X: f32 = 1e-3;

    /// The unit normal shared by all the batch's normals: `None` if there are none, one is zero,
    /// or they differ.
    pub fn plane_normal(&self) -> Option<[f32; 3]> {
        const SAME_PLANE_COS: f32 = 0.999;
        let unit = |n: &[f32; 3]| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (l > 1e-6).then(|| [n[0] / l, n[1] / l, n[2] / l])
        };
        let n0 = self.normals.first().and_then(unit)?;
        self.normals
            .iter()
            .all(|n| {
                unit(n).is_some_and(|n| n[0] * n0[0] + n[1] * n0[1] + n[2] * n0[2] > SAME_PLANE_COS)
            })
            .then_some(n0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(normals: Vec<[f32; 3]>, billboard: bool) -> RenderSubmesh {
        RenderSubmesh {
            positions: vec![[0.0; 3]; normals.len()],
            normals,
            billboard: billboard.then(|| Billboard {
                pivot: [0.0; 3],
                bone: 0,
                kind: BillboardKind::LockZ,
                scale_anim: None,
                seq_translations: Vec::new(),
            }),
            ..RenderSubmesh::default()
        }
    }

    #[test]
    fn only_flat_billboard_cards_facing_minus_x_face_away() {
        assert!(card(vec![[-1.0, 0.0, 0.0]; 4], true).billboard_card_faces_away());
        assert!(
            card(vec![[-0.98, 0.01, -0.02], [-1.02, -0.01, 0.01]], true)
                .billboard_card_faces_away()
        );
        assert!(!card(vec![[1.0, 0.0, 0.0]; 4], true).billboard_card_faces_away());
        assert!(
            !card(vec![[0.0, 0.0, -1.0]; 4], true).billboard_card_faces_away(),
            "edge-on"
        );
        assert!(
            !card(vec![[0.0; 3]; 2], true).billboard_card_faces_away(),
            "zero normals"
        );
        assert!(!card(vec![], true).billboard_card_faces_away());
        assert!(!card(vec![[-1.0, 0.0, 0.0]; 4], false).billboard_card_faces_away());
    }
}
