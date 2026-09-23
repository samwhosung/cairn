use std::fmt::Write;

use crate::bands::{sample_color, sample_float};
use crate::catalog::Light;
use crate::sample::{
    FLOAT_BAND_ROWS, INT_BAND_ROWS, SLOT_CLEAR, SLOT_CLEAR_UNDERWATER, SLOT_DEATH, SLOT_STORM,
    SLOT_STORM_UNDERWATER, blend_alpha, distance, float_band_id, int_band_id,
};
use crate::{Atmosphere, LightCatalog, Submersion};

fn rgb(x: [f32; 3]) -> String {
    format!(
        "[{:3} {:3} {:3}]",
        (x[0] * 255.0) as u8,
        (x[1] * 255.0) as u8,
        (x[2] * 255.0) as u8
    )
}

impl LightCatalog {
    /// Every band row of the clear profile [`Self::sample_smallest_sphere`] picks at `pos`, at
    /// `time`.
    pub fn debug_bands(&self, map: u32, pos: [f32; 3], time: u32) -> String {
        let Some(light) = self.pick_light(map, pos) else {
            return "(no light covers this position)\n".to_string();
        };
        match light.params[SLOT_CLEAR] {
            0 => "(no clear LightParams)\n".to_string(),
            p => self.debug_param(p, time),
        }
    }

    /// Every band row of `LightParams` record `p` at `time`.
    pub fn debug_param(&self, p: u32, time: u32) -> String {
        if p < 1 {
            return "(LightParams ids are 1-based)\n".to_string();
        }
        let mut s = String::new();
        let _ = writeln!(
            s,
            "LightParams {p} @ time {time} half-min — all int rows (sRGB 0..255):"
        );
        for b in 0..INT_BAND_ROWS {
            let key = int_band_id(p, b);
            let _ = match self
                .int_bands
                .get(&key)
                .and_then(|band| sample_color(band, time))
            {
                Some(c) => writeln!(
                    s,
                    "  int[{b:2}] = [{:3}, {:3}, {:3}]",
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8
                ),
                None => writeln!(s, "  int[{b:2}] = (unset)"),
            };
        }
        for b in 0..FLOAT_BAND_ROWS {
            let key = float_band_id(p, b);
            let _ = match self
                .float_bands
                .get(&key)
                .and_then(|band| sample_float(band, time))
            {
                Some(v) => writeln!(s, "  float[{b}] = {v:.4}"),
                None => writeln!(s, "  float[{b}] = (unset)"),
            };
        }
        s
    }

    /// The eight nearest spheres to `pos` with their weights and clear-profile colours, then
    /// [`Self::sample_smallest_sphere`] against [`Self::sample`], and the sampled water colours.
    pub fn debug_blend(&self, map: u32, pos: [f32; 3], time: u32) -> String {
        let mut rows: Vec<(f32, f32, &Light)> = self
            .lights
            .iter()
            .filter(|l| l.map == map && !l.global)
            .map(|l| {
                let d = distance(l, pos);
                (d, blend_alpha(d, l.falloff_start, l.falloff_end), l)
            })
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut s = String::new();
        let _ = writeln!(
            s,
            "Lights on map {map} near [{:.0} {:.0} {:.0}] (nearest 8) — dist | start->end | alpha | amb / sun:",
            pos[0], pos[1], pos[2]
        );
        for (d, alpha, l) in rows.iter().take(8) {
            let a = if l.params[SLOT_CLEAR] >= 1 {
                self.sample_param(l.params[SLOT_CLEAR], time)
            } else {
                Atmosphere::DEFAULT
            };
            let _ = writeln!(
                s,
                "  d={d:7.0} | {:6.0}->{:6.0} | a={alpha:.3} | amb {} sun {}",
                l.falloff_start,
                l.falloff_end,
                rgb(a.ambient),
                rgb(a.sun_diffuse)
            );
        }
        let global = self
            .lights
            .iter()
            .find(|l| l.map == map && l.global)
            .and_then(|l| {
                (l.params[SLOT_CLEAR] >= 1).then(|| self.sample_param(l.params[SLOT_CLEAR], time))
            });
        if let Some(g) = global {
            let _ = writeln!(
                s,
                "  GLOBAL (0,0,0) base      | amb {} sun {}",
                rgb(g.ambient),
                rgb(g.sun_diffuse)
            );
        }
        let picked = self.sample_smallest_sphere(map, pos, time, false);
        let blended = self.sample(map, pos, time, false, Submersion::Dry, false);
        let _ = writeln!(
            s,
            "  => pick_light : amb {} sun {}",
            rgb(picked.ambient),
            rgb(picked.sun_diffuse)
        );
        let _ = writeln!(
            s,
            "  => blended    : amb {} sun {}",
            rgb(blended.ambient),
            rgb(blended.sun_diffuse)
        );
        let _ = writeln!(
            s,
            "  => water river: shallow {} a={:.2}  deep {} a={:.2}",
            rgb(blended.water_river[0]),
            blended.water_river_alpha[0],
            rgb(blended.water_river[1]),
            blended.water_river_alpha[1],
        );
        let _ = writeln!(
            s,
            "  => water ocean: shallow {} a={:.2}  deep {} a={:.2}",
            rgb(blended.water_ocean[0]),
            blended.water_ocean_alpha[0],
            rgb(blended.water_ocean[1]),
            blended.water_ocean_alpha[1],
        );
        s
    }

    /// For each of the five profiles: the `LightParams` ids of the picked sphere and the global
    /// light, and what [`Self::sample`] resolves.
    pub fn debug_slots(&self, map: u32, pos: [f32; 3], time: u32) -> String {
        const NAMES: [&str; 5] = [
            "clear",
            "clear-underwater",
            "storm",
            "storm-underwater",
            "death",
        ];
        let picked = self.pick_light(map, pos);
        let global = self.lights.iter().find(|l| l.map == map && l.global);
        let mut s = String::new();
        let _ = writeln!(
            s,
            "Light slots on map {map} at [{:.1} {:.1} {:.1}], time {time} half-min:",
            pos[0], pos[1], pos[2]
        );
        let _ = match picked {
            Some(l) if l.global => {
                writeln!(s, "  picked sphere : (none local — the continent global)")
            }
            Some(l) => writeln!(
                s,
                "  picked sphere : local, falloff {:.0}->{:.0} yd, params {:?}",
                l.falloff_start, l.falloff_end, l.params
            ),
            None => writeln!(s, "  picked sphere : (no light covers this position)"),
        };
        if let Some(g) = global {
            let _ = writeln!(s, "  continent glob: params {:?}", g.params);
        }
        let _ = writeln!(
            s,
            "  {:<17} {:>6} {:>6} {:>9} {:>6}  {:<15} {:<15} {:<15}",
            "slot", "picked", "global", "fog_end", "frac", "fog_color", "ambient", "diffuse"
        );
        for (slot, name) in NAMES.iter().enumerate() {
            let show = |l: Option<&Light>| match l.map(|l| l.params[slot]) {
                None => "     -".to_string(),
                Some(0) => " unset".to_string(),
                Some(p) => format!("{p:>6}"),
            };
            let a = self.sample(
                map,
                pos,
                time,
                slot == SLOT_STORM || slot == SLOT_STORM_UNDERWATER,
                if slot == SLOT_CLEAR_UNDERWATER || slot == SLOT_STORM_UNDERWATER {
                    Submersion::Water
                } else {
                    Submersion::Dry
                },
                slot == SLOT_DEATH,
            );
            let _ = writeln!(
                s,
                "  {name:<17} {} {} {:>9.1} {:>6.2}  {:<15} {:<15} {:<15}",
                show(picked),
                show(global),
                a.fog_end,
                a.fog_start_frac,
                rgb(a.fog_color),
                rgb(a.ambient),
                rgb(a.sun_diffuse)
            );
        }
        s
    }
}
