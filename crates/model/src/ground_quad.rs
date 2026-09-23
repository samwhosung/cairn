use crate::RenderSubmesh;

/// A batch that is one flat quad lying level at or above z = 0 in model space: an
/// axis-aligned rectangle in XY with every vertex wholly weighted to one bone. Spell effects
/// author their ground rings this way.
#[derive(Debug, Clone, Copy)]
pub struct GroundQuad {
    pub bone: u16,
    /// Model space, at their authored height, ordered `(min_x, min_y)`, `(max_x, min_y)`,
    /// `(min_x, max_y)`, `(max_x, max_y)`.
    pub corners: [[f32; 3]; 4],
    /// The authored UV at each corner.
    pub uvs: [[f32; 2]; 4],
    /// The batch's static colour-track tint; white when it has none or the tint is animated.
    pub tint: [f32; 3],
}

const GROUND_FLAT_EPS: f32 = 0.01;

/// The highest a ground quad's plane may hover. Spell ground rings hover up to 0.52; the next
/// flat spell quads up, at 1.39, are mid-body glows.
const GROUND_HOVER_MAX: f32 = 1.0;

impl RenderSubmesh {
    /// This batch as a [`GroundQuad`], or `None` unless it is exactly that shape with its plane
    /// no higher than `GROUND_HOVER_MAX`. Billboard batches never match.
    pub fn ground_quad(&self) -> Option<GroundQuad> {
        self.ground_quad_hover(GROUND_HOVER_MAX).map(|(q, _)| q)
    }

    /// [`Self::ground_quad`] with the hover ceiling as a parameter, also returning the plane's
    /// height.
    pub fn ground_quad_hover(&self, max_hover: f32) -> Option<(GroundQuad, f32)> {
        if self.billboard.is_some()
            || self.positions.len() != 4
            || self.joints.len() != 4
            || self.weights.len() != 4
            || self.uvs.len() != 4
        {
            return None;
        }
        let hover = self.positions.iter().map(|p| p[2]).sum::<f32>() / 4.0;
        if !(-GROUND_FLAT_EPS..=max_hover).contains(&hover)
            || self
                .positions
                .iter()
                .any(|p| (p[2] - hover).abs() > GROUND_FLAT_EPS)
        {
            return None;
        }
        let bone = self.joints[0][0];
        if self
            .joints
            .iter()
            .zip(&self.weights)
            .any(|(j, w)| j[0] != bone || w[0] < 0.999)
        {
            return None;
        }
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &self.positions {
            for a in 0..2 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        let eps = [
            (max[0] - min[0]) * 1e-3 + 1e-6,
            (max[1] - min[1]) * 1e-3 + 1e-6,
        ];
        if max[0] - min[0] <= eps[0] || max[1] - min[1] <= eps[1] {
            return None;
        }
        let mut corners = [[0.0_f32; 3]; 4];
        let mut uvs = [[0.0_f32; 2]; 4];
        let mut filled = [false; 4];
        for (p, uv) in self.positions.iter().zip(&self.uvs) {
            let sx = if (p[0] - min[0]).abs() <= eps[0] {
                0
            } else if (p[0] - max[0]).abs() <= eps[0] {
                1
            } else {
                return None;
            };
            let sy = if (p[1] - min[1]).abs() <= eps[1] {
                0
            } else if (p[1] - max[1]).abs() <= eps[1] {
                1
            } else {
                return None;
            };
            let slot = sy * 2 + sx;
            if filled[slot] {
                return None;
            }
            filled[slot] = true;
            corners[slot] = *p;
            uvs[slot] = *uv;
        }
        let tint = self
            .vertex_colors
            .first()
            .map_or([1.0; 3], |c| [c[0], c[1], c[2]]);
        Some((
            GroundQuad {
                bone,
                corners,
                uvs,
                tint,
            },
            hover,
        ))
    }
}
