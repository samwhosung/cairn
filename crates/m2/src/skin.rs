use wowfile::{ByteExt, capped};

use crate::error::{Error, Result};
use crate::model::M2Model;

const VIEW_SIZE: usize = 44;
const BATCH_SIZE: usize = 24;

/// One submesh: a geoset and its triangle range.
#[derive(Debug)]
pub struct SkinSection {
    /// The geoset id, `group * 100 + variant`; character geoset selection toggles by it.
    pub id: u16,
    pub triangle_start: u16,
    pub triangle_count: u16,
}

/// One draw batch: a submesh with its material, textures and animated factors.
#[derive(Debug)]
pub struct SkinBatch {
    pub flags: u16,
    pub shader_id: u16,
    pub skin_section_index: u16,
    pub texture_combo_index: u16,
    /// Indexes [`M2Model::texture_unit_lookup`].
    pub texture_coord_combo_index: u16,
    pub material_index: u16,
    /// Indexes [`M2Model::color_alpha_tracks`] directly, `0xffff` for none.
    pub color_index: u16,
    /// Indexes [`M2Model::transparency_lookup`].
    pub weight_combo_index: u16,
    /// With `0`, the batch's weight track does not apply.
    pub texture_count: u16,
    /// Indexes [`M2Model::texture_transform_lookup`].
    pub texture_transform_combo_index: u16,
}

/// One skin profile (view) embedded in the model file.
#[derive(Debug)]
pub struct Skin {
    indices: Vec<u16>,
    triangles: Vec<u16>,
    submeshes: Vec<SkinSection>,
    batches: Vec<SkinBatch>,
}

impl Skin {
    pub fn indices(&self) -> &Vec<u16> {
        &self.indices
    }

    pub fn triangles(&self) -> &Vec<u16> {
        &self.triangles
    }

    pub fn submeshes(&self) -> &Vec<SkinSection> {
        &self.submeshes
    }

    pub fn batches(&self) -> &Vec<SkinBatch> {
        &self.batches
    }
}

impl M2Model {
    /// Reads skin profile `index` out of `bytes`, the file this model was parsed from. An index
    /// past the model's profiles is [`Error::Truncated`].
    pub fn parse_embedded_skin(&self, bytes: &[u8], index: usize) -> Result<Skin> {
        let (count, ofs) = self.views;
        if index >= count as usize {
            return Err(Error::Truncated);
        }
        let view = ofs as usize + index * VIEW_SIZE;
        let array = |o: usize| -> Result<(usize, usize)> {
            Ok((
                bytes.u32_at(o).ok_or(Error::Truncated)? as usize,
                bytes.u32_at(o + 4).ok_or(Error::Truncated)? as usize,
            ))
        };
        let (n_idx, o_idx) = array(view)?;
        let (n_tri, o_tri) = array(view + 8)?;
        let (n_sub, o_sub) = array(view + 24)?;
        let (n_bat, o_bat) = array(view + 32)?;

        let read_u16s = |n: usize, o: usize| -> Result<Vec<u16>> {
            let slice = bytes.bytes_at(o, n * 2).ok_or(Error::Truncated)?;
            Ok(slice
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect())
        };
        let indices = read_u16s(n_idx, o_idx)?;
        let triangles = read_u16s(n_tri, o_tri)?;

        let section_size = if self.version < 260 { 32 } else { 48 };
        let sub_avail = bytes.len().saturating_sub(o_sub);
        let mut submeshes = Vec::with_capacity(capped(n_sub, section_size, sub_avail));
        for i in 0..n_sub {
            let s = bytes
                .bytes_at(o_sub + i * section_size, section_size)
                .ok_or(Error::Truncated)?;
            submeshes.push(SkinSection {
                id: s.u16_at(0).ok_or(Error::Truncated)?,
                triangle_start: s.u16_at(8).ok_or(Error::Truncated)?,
                triangle_count: s.u16_at(10).ok_or(Error::Truncated)?,
            });
        }
        let bat_avail = bytes.len().saturating_sub(o_bat);
        let mut batches = Vec::with_capacity(capped(n_bat, BATCH_SIZE, bat_avail));
        for i in 0..n_bat {
            let u = bytes
                .bytes_at(o_bat + i * BATCH_SIZE, BATCH_SIZE)
                .ok_or(Error::Truncated)?;
            let at = |o: usize| u.u16_at(o).ok_or(Error::Truncated);
            batches.push(SkinBatch {
                flags: at(0)?,
                shader_id: at(2)?,
                skin_section_index: at(4)?,
                color_index: at(0x08)?,
                material_index: at(0x0a)?,
                texture_count: at(0x0e)?,
                texture_combo_index: at(0x10)?,
                texture_coord_combo_index: at(0x12)?,
                weight_combo_index: at(0x14)?,
                texture_transform_combo_index: at(0x16)?,
            });
        }
        Ok(Skin {
            indices,
            triangles,
            submeshes,
            batches,
        })
    }
}
