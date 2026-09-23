/// What a colour band row with no keys samples to, as in the client.
pub const ZERO_KEY_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// What a float band row with no keys samples to, as in the client.
pub const ZERO_KEY_SCALAR: f32 = 0.0;

/// The atmosphere at one place and time. Distances are yards; colours are sRGB in `0..=1`.
#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    /// Where fog is full. The client draws it no farther than the view distance.
    pub fog_end: f32,
    /// Where fog starts, as a fraction of the drawn fog end. It can be negative, which puts fog
    /// at the eye.
    pub fog_start_frac: f32,
    pub fog_color: [f32; 3],
    pub sun_diffuse: [f32; 3],
    /// The sun and moon discs and their glare.
    pub sun_color: [f32; 3],
    pub ambient: [f32; 3],
    /// The sky dome from the zenith down. Its rim at the horizon takes `fog_color`.
    pub sky: [[f32; 3]; 5],
    /// River and lake surface colour, `[shallow, deep]`.
    pub water_river: [[f32; 3]; 2],
    /// Ocean surface colour, `[shallow, deep]`.
    pub water_ocean: [[f32; 3]; 2],
    /// River and lake surface alpha, `[shallow, deep]`.
    pub water_river_alpha: [f32; 2],
    /// Ocean surface alpha, `[shallow, deep]`.
    pub water_ocean_alpha: [f32; 2],
    /// Bloom weight.
    pub glow: f32,
    /// How much the zone's sky takes the dawn and dusk highlight, `0..=1`.
    pub highlight_sky: f32,
    pub cloud_density: f32,
    /// `[sun glow, gradient slope, gradient base]`.
    pub cloud_colors: [[f32; 3]; 3],
}

impl Atmosphere {
    /// The atmosphere where no light applies. Its glow, highlight and water alphas also stand in
    /// for a `LightParams` id with no record.
    pub const DEFAULT: Atmosphere = Atmosphere {
        fog_end: 1000.0,
        fog_start_frac: 0.4,
        fog_color: [0.55, 0.72, 0.92],
        sun_diffuse: [1.0, 0.96, 0.86],
        sun_color: [1.0, 1.0, 0.9],
        ambient: [0.45, 0.52, 0.65],
        sky: [
            [0.30, 0.50, 0.85],
            [0.35, 0.58, 0.88],
            [0.55, 0.72, 0.92],
            [0.68, 0.80, 0.93],
            [0.78, 0.86, 0.95],
        ],
        water_river: [[0.27, 0.33, 0.14], [0.19, 0.31, 0.32]],
        water_ocean: [[0.10, 0.29, 0.34], [0.04, 0.16, 0.28]],
        water_river_alpha: [0.5, 1.0],
        water_ocean_alpha: [0.75, 1.0],
        glow: 0.5,
        highlight_sky: 0.0,
        cloud_density: 0.0,
        cloud_colors: [[1.0, 0.98, 0.9], [0.35, 0.38, 0.42], [0.75, 0.78, 0.82]],
    };

    #[must_use]
    pub fn lerp(&self, other: &Atmosphere, t: f32) -> Atmosphere {
        let f = |a: f32, b: f32| a + (b - a) * t;
        let c = |a: [f32; 3], b: [f32; 3]| [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])];
        Atmosphere {
            fog_end: f(self.fog_end, other.fog_end),
            fog_start_frac: f(self.fog_start_frac, other.fog_start_frac),
            fog_color: c(self.fog_color, other.fog_color),
            sun_diffuse: c(self.sun_diffuse, other.sun_diffuse),
            sun_color: c(self.sun_color, other.sun_color),
            ambient: c(self.ambient, other.ambient),
            sky: std::array::from_fn(|i| c(self.sky[i], other.sky[i])),
            water_river: std::array::from_fn(|i| c(self.water_river[i], other.water_river[i])),
            water_ocean: std::array::from_fn(|i| c(self.water_ocean[i], other.water_ocean[i])),
            water_river_alpha: std::array::from_fn(|i| {
                f(self.water_river_alpha[i], other.water_river_alpha[i])
            }),
            water_ocean_alpha: std::array::from_fn(|i| {
                f(self.water_ocean_alpha[i], other.water_ocean_alpha[i])
            }),
            glow: f(self.glow, other.glow),
            highlight_sky: f(self.highlight_sky, other.highlight_sky),
            cloud_density: f(self.cloud_density, other.cloud_density),
            cloud_colors: std::array::from_fn(|i| c(self.cloud_colors[i], other.cloud_colors[i])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_blends_every_field() {
        let a = Atmosphere {
            glow: 0.50,
            ..Atmosphere::DEFAULT
        };
        let b = Atmosphere {
            glow: 0.85,
            fog_end: 0.0,
            ..Atmosphere::DEFAULT
        };
        let mid = a.lerp(&b, 0.5);
        assert!((mid.glow - 0.675).abs() < 1e-6);
        assert!((mid.fog_end - 500.0).abs() < 1e-3);
        assert_eq!(mid.sky[4].map(f32::to_bits), a.sky[4].map(f32::to_bits));
    }
}
