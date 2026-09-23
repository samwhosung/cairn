use std::io::Cursor;

use m2::parse_m2;
use mpq::Chain;
use wowfile::ByteExt;

use crate::{Error, model_path};

/// An M2's authored bounds, with extents measured off its vertices and sequence boxes, in the
/// model's own yards before placement scale. The Stand box is animation 0's sequence box, else
/// the first sequence's, else the header box.
#[derive(Clone, Copy, Debug)]
pub struct M2Bounds {
    /// The header's bounding-sphere radius.
    pub sphere_radius: f32,
    /// The header's bounding box.
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub vert_max_from_origin: f32,
    /// The selection ring's radius: `sqrt(0.5 · sqrt(dx² + dy²))` over the Stand box's X and Y
    /// extents, or [`DEGENERATE_RING_FOOTPRINT`] when both are zero.
    pub ring_footprint: f32,
    /// Z of attachment 17, the base of the follow camera's pivot height; `None` without one.
    pub pivot_z: Option<f32>,
    /// The Stand box's height, floored at zero.
    pub stand_box_z: f32,
    /// The top of the Stand box minus the top of the Swim sequence's box, floored at zero; `0.0`
    /// without a Swim sequence.
    pub swim_pivot_drop: f32,
}

/// Reads an M2's bounds from the chain; `.mdx` and `.mdl` paths resolve to `.m2`.
pub fn load_m2_bounds(chain: &Chain, raw_path: &str) -> Result<M2Bounds, Error> {
    let bytes = chain.read(&model_path(raw_path)).map_err(Error::Chain)?;
    parse_m2_bounds(&bytes)
}

/// The ring footprint the client uses, instead of the formula, when a box's X and Y extents are
/// both zero.
pub const DEGENERATE_RING_FOOTPRINT: f32 = 1.2;

const SEQ_RECORD_LEN: usize = 0x44;
const SEQ_BOX_MIN: usize = 0x24;
const SEQ_BOX_MAX: usize = 0x30;

fn seq_index(bytes: &[u8], anim_id: usize) -> Option<usize> {
    let anim_count = bytes.u32_at(0x1c)? as usize;
    let lookup_count = bytes.u32_at(0x24)? as usize;
    let lookup_ofs = bytes.u32_at(0x28)? as usize;
    if anim_count == 0 || anim_id >= lookup_count {
        return None;
    }
    match bytes.u16_at(lookup_ofs.checked_add(anim_id * 2)?) {
        Some(i) if (i as usize) != 0xffff && (i as usize) < anim_count => Some(i as usize),
        _ => None,
    }
}

fn seq_record(bytes: &[u8], idx: usize) -> Option<usize> {
    let anim_ofs = bytes.u32_at(0x20)? as usize;
    anim_ofs.checked_add(idx * SEQ_RECORD_LEN)
}

fn stand_record(bytes: &[u8]) -> Option<usize> {
    if bytes.u32_at(0x1c)? as usize == 0 {
        return None;
    }
    seq_record(bytes, seq_index(bytes, 0).unwrap_or(0))
}

fn seq_max_z(bytes: &[u8], anim_id: usize) -> Option<f32> {
    let rec = seq_record(bytes, seq_index(bytes, anim_id)?)?;
    bytes.f32_at(rec + SEQ_BOX_MAX + 8)
}

fn stand_box_extents(bytes: &[u8]) -> Option<(f32, f32, f32)> {
    let rec = stand_record(bytes)?;
    let dx = bytes.f32_at(rec + SEQ_BOX_MAX)? - bytes.f32_at(rec + SEQ_BOX_MIN)?;
    let dy = bytes.f32_at(rec + SEQ_BOX_MAX + 4)? - bytes.f32_at(rec + SEQ_BOX_MIN + 4)?;
    let dz = bytes.f32_at(rec + SEQ_BOX_MAX + 8)? - bytes.f32_at(rec + SEQ_BOX_MIN + 8)?;
    Some((dx, dy, dz))
}

const SWIM_ANIM_ID: usize = 42;

fn swim_pivot_drop(bytes: &[u8]) -> f32 {
    let Some(stand_z) = stand_record(bytes).and_then(|rec| bytes.f32_at(rec + SEQ_BOX_MAX + 8))
    else {
        return 0.0;
    };
    let Some(swim_z) = seq_max_z(bytes, SWIM_ANIM_ID) else {
        return 0.0;
    };
    (stand_z - swim_z).max(0.0)
}

/// Reads an M2's bounds from the file's bytes.
pub fn parse_m2_bounds(bytes: &[u8]) -> Result<M2Bounds, Error> {
    let format = parse_m2(&mut Cursor::new(bytes)).map_err(Error::M2)?;
    let model = format.model();
    let h = &model.bounds;
    let vert_max_from_origin = model
        .vertices
        .iter()
        .map(|v| {
            let p = v.position;
            (p.x * p.x + p.y * p.y + p.z * p.z).sqrt()
        })
        .fold(0.0_f32, f32::max);
    let (rx, ry, rz) = stand_box_extents(bytes).unwrap_or((
        h.bounding_box_max[0] - h.bounding_box_min[0],
        h.bounding_box_max[1] - h.bounding_box_min[1],
        h.bounding_box_max[2] - h.bounding_box_min[2],
    ));
    let ring_footprint = if rx == 0.0 && ry == 0.0 {
        DEGENERATE_RING_FOOTPRINT
    } else {
        (0.5 * (rx * rx + ry * ry).sqrt()).sqrt()
    };
    Ok(M2Bounds {
        sphere_radius: h.bounding_sphere_radius,
        bbox_min: h.bounding_box_min,
        bbox_max: h.bounding_box_max,
        vert_max_from_origin,
        ring_footprint,
        pivot_z: model.pivot_attach_z,
        stand_box_z: rz.max(0.0),
        swim_pivot_drop: swim_pivot_drop(bytes),
    })
}
