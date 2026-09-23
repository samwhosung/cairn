use wowfile::ByteExt;

use crate::model::M2Camera;
use crate::parse::{bytes_from, rd_vec3};
use crate::track::{track_spline_f32, track_spline_vec3};

const CAMERAS: usize = 0x124;
const CAMERA_LOOKUP: usize = 0x12c;
const CAMERA_SIZE: usize = 0x7c;

/// Reads an MD20 file's cameras without parsing the rest of the model. Never fails: a table cut
/// short yields the whole records that fit, and a file too short for the header yields none.
pub fn parse_cameras(b: &[u8]) -> Vec<M2Camera> {
    let (Some(count), Some(ofs)) = (b.u32_at(CAMERAS), b.u32_at(CAMERAS + 4)) else {
        return Vec::new();
    };
    let ofs = ofs as usize;
    bytes_from(b, ofs)
        .as_chunks::<CAMERA_SIZE>()
        .0
        .iter()
        .take(count as usize)
        .enumerate()
        .map(|(i, r)| {
            let rec = ofs + i * CAMERA_SIZE;
            let r = r.as_slice();
            let f32_at = |o: usize| r.f32_at(o).unwrap_or_default();
            let vec3_at = |o: usize| rd_vec3(r, o).unwrap_or_default();
            M2Camera {
                camera_type: r.i32_at(0).unwrap_or_default(),
                fov: f32_at(0x04),
                far_clip: f32_at(0x08),
                near_clip: f32_at(0x0c),
                positions: track_spline_vec3(b, rec + 0x10),
                position_base: vec3_at(0x2c),
                target: track_spline_vec3(b, rec + 0x38),
                target_base: vec3_at(0x54),
                roll: track_spline_f32(b, rec + 0x60),
            }
        })
        .collect()
}

/// Reads an MD20 file's camera lookup, a camera purpose to its index in the cameras. Never
/// fails: a table cut short yields the entries that fit.
pub fn parse_camera_lookup(b: &[u8]) -> Vec<u16> {
    let (Some(count), Some(ofs)) = (b.u32_at(CAMERA_LOOKUP), b.u32_at(CAMERA_LOOKUP + 4)) else {
        return Vec::new();
    };
    bytes_from(b, ofs as usize)
        .as_chunks::<2>()
        .0
        .iter()
        .take(count as usize)
        .map(|&c| u16::from_le_bytes(c))
        .collect()
}
