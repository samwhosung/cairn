use bevy::math::{Vec3, Vec4};

#[allow(clippy::manual_midpoint, reason = "a band coefficient, not a midpoint")]
pub fn sh_probe_coeffs(ambient: [f32; 3], lobes: &[(Vec3, [f32; 3])]) -> [Vec4; 7] {
    const K: f32 = 4.0 / 17.0;
    let mut c = [Vec4::ZERO; 7];
    for (ch, a) in ambient.iter().enumerate() {
        c[ch].w = *a;
    }
    c[6].w = 1.0;
    for (u, col) in lobes {
        let u = u.normalize_or_zero();
        let (ux, uy, uz) = (u.x, u.y, u.z);
        let dc = K * (0.375 + 0.9375 * (ux * ux + uy * uy));
        let z2 = 1.875 * K * (uz * uz - 0.5 * (ux * ux + uy * uy));
        let x2y2 = 0.9375 * K * (ux * ux - uy * uy);
        for ch in 0..3 {
            let s = col[ch];
            c[ch] += Vec4::new(2.0 * K * s * ux, 2.0 * K * s * uy, 2.0 * K * s * uz, s * dc);
            c[3 + ch] += Vec4::new(
                3.75 * K * s * ux * uy,
                3.75 * K * s * uy * uz,
                s * z2,
                3.75 * K * s * ux * uz,
            );
            c[6][ch] += s * x2y2;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(c: &[Vec4; 7], n: Vec3) -> [f32; 3] {
        let quad = Vec4::new(n.x * n.y, n.y * n.z, n.z * n.z, n.x * n.z);
        let n1 = n.extend(1.0);
        let x2y2 = n.x * n.x - n.y * n.y;
        [0usize, 1, 2].map(|ch| c[ch].dot(n1) + c[3 + ch].dot(quad) + c[6][ch] * x2y2)
    }

    #[test]
    fn the_lobe_is_the_closed_form_at_every_normal() {
        let ambient = [0.1, 0.2, 0.3];
        let colour = [0.9, 0.6, 0.3];
        let u = Vec3::new(0.3, 0.8, -0.52).normalize();
        let c = sh_probe_coeffs(ambient, &[(u, colour)]);
        let side = u.cross(Vec3::Y).normalize();
        for n in [u, -u, side, (u + side).normalize(), Vec3::X, Vec3::Z] {
            let mu = n.dot(u);
            let f = (4.0 / 17.0) * (0.375 + 2.0 * mu + 1.875 * mu * mu);
            let got = eval(&c, n);
            for ch in 0..3 {
                let want = ambient[ch] + colour[ch] * f;
                assert!((got[ch] - want).abs() < 1e-5, "{n}: {got:?} vs {want}");
            }
        }
    }
}
