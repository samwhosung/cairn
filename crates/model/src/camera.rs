/// An M2 camera at one instant, in model space (WoW axes); angles in radians.
#[derive(Debug, Clone, Copy)]
pub struct M2PortraitCamera {
    /// Field of view, measured diagonally: the client projects with the half-angle
    /// `(fov / 2) / √(aspect² + 1)`.
    pub fov: f32,
    pub far_clip: f32,
    pub near_clip: f32,
    /// The eye: the record's position base plus its position track.
    pub position: [f32; 3],
    /// The look target: the target base plus the target track.
    pub target: [f32; 3],
    /// Roll about the view axis; `0` when the roll track has no key.
    pub roll: f32,
}

/// The camera the client renders a unit portrait through: the one `cameraLookup[0]` names, at
/// its first keys. `None` when that entry is missing or names no camera (`0xffff` included).
pub fn parse_m2_portrait_camera(bytes: &[u8]) -> Option<M2PortraitCamera> {
    let idx = *m2::parse_camera_lookup(bytes).first()? as usize;
    parse_m2_camera(bytes, idx)
}

/// The camera at `index` in the camera table, at its first keys; `None` when out of range. A
/// camera whose tracks move is sampled with [`M2PaneCamera::at`] instead.
pub fn parse_m2_camera(bytes: &[u8], index: usize) -> Option<M2PortraitCamera> {
    let cam = m2::parse_cameras(bytes).into_iter().nth(index)?;
    let base_plus_key = |base: [f32; 3], track: &m2::M2Vec3SplineTrack| {
        let k = track.keys.first().map_or([0.0; 3], |(_, k)| k.value);
        [base[0] + k[0], base[1] + k[1], base[2] + k[2]]
    };
    Some(M2PortraitCamera {
        fov: cam.fov,
        far_clip: cam.far_clip,
        near_clip: cam.near_clip,
        position: base_plus_key(cam.position_base, &cam.positions),
        target: base_plus_key(cam.target_base, &cam.target),
        roll: cam.roll.keys.first().map_or(0.0, |(_, k)| k.value),
    })
}

/// One entry of the camera table, which `Model:SetCamera(n)` selects by raw index, never
/// through `cameraLookup`.
#[derive(Debug, Clone)]
pub struct M2PaneCamera {
    /// The record's type word. The client selects by index, never by type.
    pub camera_type: i32,
    /// The camera at rest: its bases plus each track's first key.
    pub still: M2PortraitCamera,
    /// The tracks, kept only when one of them moves (holds two or more distinct keys).
    pub tracks: Option<Box<M2CameraTracks>>,
}

/// A moving camera's tracks and bases: the eye is `position_base + positions(t)`, the target
/// `target_base + target(t)`, the roll `roll(t)`.
#[derive(Debug, Clone)]
pub struct M2CameraTracks {
    pub positions: m2::M2Vec3SplineTrack,
    pub position_base: [f32; 3],
    pub target: m2::M2Vec3SplineTrack,
    pub target_base: [f32; 3],
    pub roll: m2::M2ScalarSplineTrack,
}

impl M2PaneCamera {
    /// The camera at `ms` on the file's animation timeline: [`Self::still`] when nothing moves,
    /// else the tracks sampled by [`m2::M2Track::sample_ms`].
    pub fn at(&self, ms: u32) -> M2PortraitCamera {
        let Some(t) = self.tracks.as_deref() else {
            return self.still;
        };
        let add = |b: [f32; 3], v: Option<[f32; 3]>| {
            let v = v.unwrap_or([0.0; 3]);
            [b[0] + v[0], b[1] + v[1], b[2] + v[2]]
        };
        M2PortraitCamera {
            position: add(t.position_base, t.positions.sample_ms(ms)),
            target: add(t.target_base, t.target.sample_ms(ms)),
            roll: t.roll.sample_ms(ms).unwrap_or(self.still.roll),
            ..self.still
        }
    }
}

/// The whole camera table, in file order; empty for a model without cameras.
#[allow(clippy::float_cmp)]
pub fn parse_m2_pane_cameras(bytes: &[u8]) -> Vec<M2PaneCamera> {
    m2::parse_cameras(bytes)
        .into_iter()
        .map(|cam| {
            let key0 =
                |t: &m2::M2Vec3SplineTrack| t.keys.first().map_or([0.0; 3], |(_, k)| k.value);
            let base_plus = |b: [f32; 3], t: &m2::M2Vec3SplineTrack| {
                let k = key0(t);
                [b[0] + k[0], b[1] + k[1], b[2] + k[2]]
            };
            let still = M2PortraitCamera {
                fov: cam.fov,
                far_clip: cam.far_clip,
                near_clip: cam.near_clip,
                position: base_plus(cam.position_base, &cam.positions),
                target: base_plus(cam.target_base, &cam.target),
                roll: cam.roll.keys.first().map_or(0.0, |(_, k)| k.value),
            };
            let moves = |n: usize, same: bool| n > 1 && !same;
            let v3_moves = |t: &m2::M2Vec3SplineTrack| {
                let first = key0(t);
                moves(t.keys.len(), t.keys.iter().all(|(_, k)| k.value == first))
            };
            let roll_moves = {
                let first = cam.roll.keys.first().map_or(0.0, |(_, k)| k.value);
                moves(
                    cam.roll.keys.len(),
                    cam.roll.keys.iter().all(|(_, k)| k.value == first),
                )
            };
            let tracks =
                (v3_moves(&cam.positions) || v3_moves(&cam.target) || roll_moves).then(|| {
                    Box::new(M2CameraTracks {
                        positions: cam.positions,
                        position_base: cam.position_base,
                        target: cam.target,
                        target_base: cam.target_base,
                        roll: cam.roll,
                    })
                });
            M2PaneCamera {
                camera_type: cam.camera_type,
                still,
                tracks,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_too_short_for_the_tables_has_no_camera() {
        assert!(parse_m2_portrait_camera(&[0u8; 16]).is_none());
        assert!(parse_m2_camera(&[0u8; 16], 0).is_none());
        assert!(parse_m2_pane_cameras(&[0u8; 16]).is_empty());
    }
}
