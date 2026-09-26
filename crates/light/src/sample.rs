use crate::bands::{sample_color, sample_float};
use crate::catalog::{DBC_UNITS_PER_YARD, Light};
use crate::{Atmosphere, LightCatalog, Submersion, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};

pub(crate) const SLOT_CLEAR: usize = 0;
pub(crate) const SLOT_CLEAR_UNDERWATER: usize = 1;
pub(crate) const SLOT_STORM: usize = 2;
pub(crate) const SLOT_STORM_UNDERWATER: usize = 3;
pub(crate) const SLOT_DEATH: usize = 4;

const FALLBACK_LIGHT_ID: u32 = 1;

pub(crate) const INT_BAND_ROWS: u32 = 18;
pub(crate) const FLOAT_BAND_ROWS: u32 = 6;
const INT_BAND_DIFFUSE: u32 = 0;
const INT_BAND_AMBIENT: u32 = 1;
const INT_BAND_SKY: u32 = 2;
const INT_BAND_FOG: u32 = 7;
const INT_BAND_SUN: u32 = 9;
const INT_BAND_CLOUD_SUN: u32 = 10;
const INT_BAND_CLOUD_SLOPE: u32 = 11;
const INT_BAND_CLOUD_BASE: u32 = 12;
const INT_BAND_OCEAN_SHALLOW: u32 = 14;
const INT_BAND_OCEAN_DEEP: u32 = 15;
const INT_BAND_RIVER_SHALLOW: u32 = 16;
const INT_BAND_RIVER_DEEP: u32 = 17;
const FLOAT_BAND_FOG_END: u32 = 0;
const FLOAT_BAND_FOG_START: u32 = 1;
const FLOAT_BAND_CLOUD_DENSITY: u32 = 3;

pub(crate) fn int_band_id(params_id: u32, row: u32) -> Option<u32> {
    params_id
        .checked_sub(1)?
        .checked_mul(INT_BAND_ROWS)?
        .checked_add(row + 1)
}

pub(crate) fn float_band_id(params_id: u32, row: u32) -> Option<u32> {
    params_id
        .checked_sub(1)?
        .checked_mul(FLOAT_BAND_ROWS)?
        .checked_add(row + 1)
}

pub(crate) fn weather_slot(ghost: bool, stormy: bool, underwater: bool) -> usize {
    if ghost {
        return SLOT_DEATH;
    }
    match (stormy, underwater) {
        (false, false) => SLOT_CLEAR,
        (false, true) => SLOT_CLEAR_UNDERWATER,
        (true, false) => SLOT_STORM,
        (true, true) => SLOT_STORM_UNDERWATER,
    }
}

pub(crate) fn blend_alpha(dist: f32, start: f32, end: f32) -> f32 {
    if dist >= end {
        0.0
    } else if dist <= start || end <= start {
        1.0
    } else {
        (end - dist) / (end - start)
    }
}

pub(crate) fn distance(l: &Light, pos: [f32; 3]) -> f32 {
    (0..3)
        .map(|i| (l.pos[i] - pos[i]).powi(2))
        .sum::<f32>()
        .sqrt()
}

impl LightCatalog {
    /// The atmosphere the client draws at `pos` (world yards) on `map`, at `time` in half-minutes
    /// (1440 is noon).
    ///
    /// Magma and slime take a fixed record, even for a ghost. Otherwise the map's global light is
    /// the base, and every sphere containing `pos` is blended over it by its falloff weight,
    /// farthest first. `ghost` picks each light's death profile; a profile a light leaves unset
    /// falls back to its clear one.
    pub fn sample(
        &self,
        map: u32,
        pos: [f32; 3],
        time: u32,
        stormy: bool,
        submersion: Submersion,
        ghost: bool,
    ) -> Atmosphere {
        if let Some(fixed) = self.fixed(submersion, time) {
            return fixed;
        }
        let slot = weather_slot(ghost, stormy, submersion.is_water());
        let atmo_of = |l: &Light| self.profile(l, slot, time);

        let map_has_no_light = !self.lights.iter().any(|l| l.map == map);
        let mut acc = self
            .lights
            .iter()
            .find(|l| l.map == map && l.global)
            .or_else(|| {
                map_has_no_light
                    .then(|| self.lights.iter().find(|l| l.id == FALLBACK_LIGHT_ID))
                    .flatten()
            })
            .and_then(atmo_of)
            .unwrap_or(Atmosphere::DEFAULT);

        let mut locals: Vec<(f32, &Light)> = self
            .lights
            .iter()
            .filter(|l| l.map == map && !l.global)
            .filter_map(|l| {
                let d = distance(l, pos);
                (d <= l.falloff_end).then_some((d, l))
            })
            .collect();
        locals.sort_by(|a, b| b.0.total_cmp(&a.0));

        for (dist, l) in locals {
            if let Some(atmo) = atmo_of(l) {
                acc = acc.lerp(&atmo, blend_alpha(dist, l.falloff_start, l.falloff_end));
            }
        }
        acc
    }

    /// The atmosphere of the smallest sphere containing `pos`, with no blending: an approximation
    /// of [`Self::sample`], dry and alive. A light with no storm profile uses its clear one.
    pub fn sample_smallest_sphere(
        &self,
        map: u32,
        pos: [f32; 3],
        time: u32,
        stormy: bool,
    ) -> Atmosphere {
        let slot = if stormy { SLOT_STORM } else { SLOT_CLEAR };
        self.pick_light(map, pos)
            .and_then(|light| self.profile(light, slot, time))
            .unwrap_or(Atmosphere::DEFAULT)
    }

    /// The atmosphere of `Light.dbc` record `light` alone, at full weight wherever the camera is,
    /// in the profile [`Self::sample`] picks for the weather.
    pub fn sample_light(
        &self,
        light: u32,
        time: u32,
        stormy: bool,
        submersion: Submersion,
        ghost: bool,
    ) -> Atmosphere {
        if let Some(fixed) = self.fixed(submersion, time) {
            return fixed;
        }
        let slot = weather_slot(ghost, stormy, submersion.is_water());
        self.lights
            .iter()
            .find(|l| l.id == light)
            .and_then(|l| self.profile(l, slot, time))
            .unwrap_or(Atmosphere::DEFAULT)
    }

    /// The `Light.dbc` id of the light [`Self::sample_smallest_sphere`] samples at `pos` on `map`.
    pub fn light_at(&self, map: u32, pos: [f32; 3]) -> Option<u32> {
        self.pick_light(map, pos).map(|l| l.id)
    }

    fn fixed(&self, submersion: Submersion, time: u32) -> Option<Atmosphere> {
        let p = submersion.fixed_param()?;
        self.has_bands(p).then(|| self.sample_param(p, time))
    }

    fn profile(&self, l: &Light, slot: usize, time: u32) -> Option<Atmosphere> {
        let param = match l.params[slot] {
            0 => l.params[SLOT_CLEAR],
            p => p,
        };
        (param >= 1).then(|| self.sample_param(param, time))
    }

    /// `LightParams` record `p` at `time`, or `None` when the chain has no bands for it.
    pub fn sample_params_id(&self, p: u32, time: u32) -> Option<Atmosphere> {
        (p >= 1 && self.has_bands(p)).then(|| self.sample_param(p, time))
    }

    /// The model path, lowercased, of the sky a ghost sees at `pos`, from the death profile of the
    /// light [`Self::sample_smallest_sphere`] picks. The client has no fallback to another profile
    /// here.
    pub fn ghost_skybox(&self, map: u32, pos: [f32; 3]) -> Option<&str> {
        let light = self.pick_light(map, pos)?;
        self.skybox_models
            .get(&light.params[SLOT_DEATH])
            .map(String::as_str)
    }

    pub(crate) fn pick_light(&self, map: u32, pos: [f32; 3]) -> Option<&Light> {
        let mut local: Option<&Light> = None;
        let mut global: Option<&Light> = None;
        let mut any = false;
        for l in self.lights.iter().filter(|l| l.map == map) {
            any = true;
            if l.global {
                global = Some(l);
                continue;
            }
            let d2 = (0..3).map(|i| (l.pos[i] - pos[i]).powi(2)).sum::<f32>();
            if d2 <= l.falloff_end * l.falloff_end
                && local.is_none_or(|b| l.falloff_end < b.falloff_end)
            {
                local = Some(l);
            }
        }
        local.or(global).or_else(|| {
            (!any)
                .then(|| self.lights.iter().find(|l| l.id == FALLBACK_LIGHT_ID))
                .flatten()
        })
    }

    fn has_bands(&self, p: u32) -> bool {
        int_band_id(p, INT_BAND_FOG).is_some_and(|id| self.int_bands.contains_key(&id))
    }

    pub(crate) fn sample_param(&self, p: u32, t: u32) -> Atmosphere {
        let float = |row: u32| {
            float_band_id(p, row)
                .and_then(|id| self.float_bands.get(&id))
                .and_then(|b| sample_float(b, t))
        };
        let col = |row: u32| {
            int_band_id(p, row)
                .and_then(|id| self.int_bands.get(&id))
                .and_then(|b| sample_color(b, t))
                .unwrap_or(ZERO_KEY_COLOR)
        };
        let d = Atmosphere::DEFAULT;
        let water_alpha = self.water_alpha.get(&p);
        Atmosphere {
            fog_end: float(FLOAT_BAND_FOG_END).map_or(ZERO_KEY_SCALAR, |v| v / DBC_UNITS_PER_YARD),
            fog_start_frac: float(FLOAT_BAND_FOG_START).unwrap_or(ZERO_KEY_SCALAR),
            fog_color: col(INT_BAND_FOG),
            sun_diffuse: col(INT_BAND_DIFFUSE),
            sun_color: col(INT_BAND_SUN),
            ambient: col(INT_BAND_AMBIENT),
            sky: std::array::from_fn(|i| col(INT_BAND_SKY + i as u32)),
            water_river: [col(INT_BAND_RIVER_SHALLOW), col(INT_BAND_RIVER_DEEP)],
            water_ocean: [col(INT_BAND_OCEAN_SHALLOW), col(INT_BAND_OCEAN_DEEP)],
            water_river_alpha: water_alpha.map_or(d.water_river_alpha, |a| [a[0], a[1]]),
            water_ocean_alpha: water_alpha.map_or(d.water_ocean_alpha, |a| [a[2], a[3]]),
            glow: self.glow.get(&p).copied().unwrap_or(d.glow),
            highlight_sky: self
                .highlight_sky
                .get(&p)
                .copied()
                .unwrap_or(d.highlight_sky),
            cloud_density: float(FLOAT_BAND_CLOUD_DENSITY).unwrap_or(ZERO_KEY_SCALAR),
            cloud_colors: [
                col(INT_BAND_CLOUD_SUN),
                col(INT_BAND_CLOUD_SLOPE),
                col(INT_BAND_CLOUD_BASE),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_ids_past_u32_max_are_none() {
        assert_eq!(int_band_id(12, INT_BAND_FOG), Some(206));
        assert_eq!(float_band_id(12, FLOAT_BAND_FOG_END), Some(67));
        assert_eq!(int_band_id(0, 0), None);
        assert_eq!(int_band_id(u32::MAX / INT_BAND_ROWS + 2, 0), None);
        assert_eq!(float_band_id(u32::MAX, 0), None);
    }
}
