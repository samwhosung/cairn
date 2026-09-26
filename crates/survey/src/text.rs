use std::fmt::Write as _;

use crate::scan::TEXELS_PER_CHUNK;
use crate::{Model, Scales, Survey, Tally, Zone};

const CHUNK_YARDS: f64 = 100.0 / 3.0;
const CELLS_PER_CHUNK: f64 = 64.0;
const CELL_YARDS: f64 = CHUNK_YARDS / 8.0;
const ZONE_ROW_TOP: usize = 5;
const BESIDE_TOP: usize = 8;
const MIN_BESIDE_SHARE: f64 = 0.01;
const SAME_SCALE: f32 = 0.005;

pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(crate) fn stem(path: &str) -> &str {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

pub(crate) fn picture(m: &Model) -> String {
    format!("models/{}.png", m.key)
}

pub(crate) fn swatch(key: &str) -> String {
    format!("ground/{key}.png")
}

pub(crate) fn sky(z: &Zone) -> String {
    format!("zones/{}-sky.png", z.key)
}

pub(crate) fn size(bounds: Option<[[f32; 3]; 2]>) -> String {
    match bounds {
        None => "size unknown".to_owned(),
        Some([lo, hi]) => format!(
            "{:.1} yd tall, {:.1} x {:.1} across",
            hi[2] - lo[2],
            hi[0] - lo[0],
            hi[1] - lo[1]
        ),
    }
}

fn one_scale(s: &Scales) -> bool {
    (s.most - s.least).abs() < SAME_SCALE
}

fn scales(m: &Model) -> String {
    match &m.scales {
        None => String::new(),
        Some(s) if one_scale(s) => format!("{:.2}", s.least),
        Some(s) => format!("{:.2} to {:.2}", s.least, s.most),
    }
}

fn placed(t: Tally) -> String {
    match (t.on_ground, t.in_buildings) {
        (g, 0) => thousands(g.into()),
        (0, i) => format!("{} inside buildings", thousands(i.into())),
        (g, i) => format!(
            "{} ({} inside buildings)",
            thousands((g + i).into()),
            thousands(i.into())
        ),
    }
}

pub(crate) fn models_tsv(inv: &Survey) -> String {
    let mut out = String::from("kind\tpath\tsize\tplaced\tscales\tzones\tpicture\n");
    for m in &inv.models {
        let zones: Vec<String> = m
            .zones
            .iter()
            .map(|&t| format!("{} {}", inv.zones[t.index].name, placed(t)))
            .collect();
        let whole = Tally {
            index: 0,
            on_ground: m.on_ground,
            in_buildings: m.in_buildings,
        };
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            m.kind,
            m.path,
            size(m.bounds),
            placed(whole),
            scales(m),
            zones.join(", "),
            picture(m)
        );
    }
    out
}

pub(crate) fn share(part: f64, whole: f64) -> String {
    let p = 100.0 * part / whole.max(1.0);
    if p >= 9.95 {
        format!("{p:.0}%")
    } else if p >= 0.095 {
        format!("{p:.1}%")
    } else {
        "under 0.1%".to_owned()
    }
}

pub(crate) fn zone_texels(z: &Zone) -> f64 {
    f64::from(z.chunks) * TEXELS_PER_CHUNK as f64
}

pub(crate) fn ground_tsv(inv: &Survey) -> String {
    let mut out = String::from("kind\tpath\tchunks\tzones\tbeside\tswatch\n");
    for g in &inv.grounds {
        let zones: Vec<String> = g
            .zones
            .iter()
            .map(|&(z, t)| {
                let zone = &inv.zones[z];
                format!("{} {}", zone.name, share(t, zone_texels(zone)))
            })
            .collect();
        let beside: Vec<String> = g
            .beside
            .iter()
            .filter(|(_, s)| *s >= MIN_BESIDE_SHARE)
            .take(BESIDE_TOP)
            .map(|&(o, s)| format!("{} {:.0}%", stem(&inv.grounds[o].path), 100.0 * s))
            .collect();
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            g.kind,
            g.path,
            thousands(g.chunks.into()),
            zones.join(", "),
            beside.join(", "),
            swatch(&g.key)
        );
    }
    out
}

/// A zone's music and ambience, which the maps don't hold: named by the caller from the sound
/// tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZoneSound {
    /// `ZoneMusic`'s set name.
    pub music: String,
    /// Lines naming the music by day and by night and the files each picks from.
    pub music_lines: Vec<String>,
    /// The day ambience's name.
    pub ambience: String,
    pub ambience_lines: Vec<String>,
}

pub(crate) fn zones_tsv(inv: &Survey, sounds: &dyn Fn(&Zone) -> ZoneSound) -> String {
    let mut out =
        String::from("zone\tmap\tchunks\twater\tmusic\tambience\tground\tmodels\tsky\tfile\n");
    for z in &inv.zones {
        let sound = sounds(z);
        let ground: Vec<String> = z
            .grounds
            .iter()
            .take(ZONE_ROW_TOP)
            .map(|&(g, t)| {
                format!(
                    "{} {}",
                    stem(&inv.grounds[g].path),
                    share(t, zone_texels(z))
                )
            })
            .collect();
        let models: Vec<String> = z
            .models
            .iter()
            .take(ZONE_ROW_TOP)
            .map(|t| {
                format!(
                    "{} {}",
                    stem(&inv.models[t.index].path),
                    thousands(t.placed().into())
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\tzones/{}.txt",
            z.name,
            z.map_directory,
            thousands(z.chunks.into()),
            water(z),
            sound.music,
            sound.ambience,
            ground.join(", "),
            models.join(", "),
            if z.heart.is_some() {
                sky(z)
            } else {
                String::new()
            },
            z.key
        );
    }
    out
}

fn water(z: &Zone) -> String {
    let wet: Vec<String> = z
        .wet_cells
        .by_kind()
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(kind, n)| {
            let cells = f64::from(z.chunks) * CELLS_PER_CHUNK;
            format!("{kind} {}", share(f64::from(n), cells))
        })
        .collect();
    if wet.is_empty() {
        "none".to_owned()
    } else {
        wet.join(", ")
    }
}

pub(crate) fn model_txt(inv: &Survey, m: &Model, px_per_yard: Option<f32>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", m.path);
    let what = if m.building {
        "building (a WMO)".to_owned()
    } else {
        format!("{} (from its name)", m.kind)
    };
    let _ = writeln!(out, "kind: {what}");
    let _ = writeln!(out, "size at scale 1: {}", size(m.bounds));
    if let Some([[x0, y0, z0], [x1, y1, z1]]) = m.bounds {
        let _ = writeln!(
            out,
            "  its box: {x0:.2} to {x1:.2} forward (x), {y0:.2} to {y1:.2} left (y), {z0:.2} to {z1:.2} up (z) of its origin, which stands on the ground"
        );
    }
    let scale_note = px_per_yard.map_or(String::new(), |p| format!(", {p:.1} px a yard"));
    let _ = writeln!(
        out,
        "picture: {}.png{scale_note}; the figure beside it is a player's height, 2.03 yd",
        stem(&picture(m))
    );
    if !m.mesh {
        let _ = writeln!(
            out,
            "  it has no mesh: it only emits particles, light or sound, so its picture, drawn at rest, holds only the figure"
        );
    }
    let _ = writeln!(
        out,
        "placed: {} on the ground, {} inside buildings",
        thousands(m.on_ground.into()),
        thousands(m.in_buildings.into())
    );
    match &m.scales {
        Some(s) if one_scale(s) => {
            let _ = writeln!(out, "scale: always {:.2}", s.least);
        }
        Some(s) => {
            let _ = writeln!(
                out,
                "scales: {:.2} to {:.2}; a tenth below {:.2}, half below {:.2}, a tenth above {:.2}",
                s.least, s.most, s.p10, s.p50, s.p90
            );
        }
        None => {}
    }
    let _ = writeln!(out, "where:");
    for &t in &m.zones {
        let zone = &inv.zones[t.index];
        let places: Vec<String> = m
            .places
            .iter()
            .filter(|p| p.zone == t.index)
            .map(|p| format!("{} {}", p.area, thousands(p.placements.into())))
            .collect();
        let _ = writeln!(
            out,
            "  {} ({}): {} — {}",
            zone.name,
            zone.map_directory,
            placed(t),
            places.join(", ")
        );
    }
    let _ = writeln!(out, "first placements (map, x y z, heading, scale):");
    for e in &m.examples {
        let inside = e
            .building
            .as_ref()
            .map_or(String::new(), |b| format!(", inside {b}, which stands at"));
        let [x, y, z] = e.position;
        let _ = writeln!(
            out,
            "  {}{inside} {x:.1} {y:.1} {z:.1}, {:.0}°, {:.2}",
            e.map_directory, e.heading, e.scale
        );
    }
    out
}

pub(crate) fn ground_txt(inv: &Survey, g: usize) -> String {
    let ground = &inv.grounds[g];
    let texels_per_chunk = TEXELS_PER_CHUNK as f64;
    let mut out = String::new();
    let _ = writeln!(out, "{}", ground.path);
    let _ = writeln!(out, "kind: {} (from its name)", ground.kind);
    let _ = writeln!(
        out,
        "swatch: {}.png, the texture twice across and twice down: the ground repeats it every {:.2} yd, so the swatch is {:.1} yd across",
        stem(&swatch(&ground.key)),
        CELL_YARDS,
        2.0 * CELL_YARDS
    );
    let _ = writeln!(
        out,
        "painted: in {} chunks, showing on {} chunks' worth of ground ({} yd²)",
        thousands(ground.chunks.into()),
        thousands((ground.texels / texels_per_chunk).round() as u64),
        thousands((ground.texels / texels_per_chunk * CHUNK_YARDS * CHUNK_YARDS).round() as u64)
    );
    let _ = writeln!(out, "by zone, as a share of the zone's ground:");
    for &(z, t) in &ground.zones {
        let zone = &inv.zones[z];
        let _ = writeln!(
            out,
            "  {} ({}): {}",
            zone.name,
            zone.map_directory,
            share(t, zone_texels(zone))
        );
    }
    let _ = writeln!(
        out,
        "painted beside, as the share of its ground in chunks that paint that one too:"
    );
    for &(o, s) in ground.beside.iter().filter(|(_, s)| *s >= MIN_BESIDE_SHARE) {
        let other = &inv.grounds[o];
        let _ = writeln!(out, "  {} ({}) {:.0}%", other.path, other.kind, 100.0 * s);
    }
    out
}

pub(crate) fn zone_txt(inv: &Survey, z: &Zone, sound: &ZoneSound, sky: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", z.name);
    let _ = writeln!(
        out,
        "map: {} (Map.dbc {}); AreaTable {}",
        z.map_directory, z.map, z.area
    );
    let yards = f64::from(z.chunks) * CHUNK_YARDS * CHUNK_YARDS;
    let tiles = z.tiles.map_or(String::new(), |t| {
        format!(" in tiles {}..={} by {}..={}", t.x0, t.x1, t.y0, t.y1)
    });
    let _ = writeln!(
        out,
        "ground: {} chunks, {} yd²{tiles}",
        thousands(z.chunks.into()),
        thousands(yards.round() as u64)
    );
    if let Some([x, y, h]) = z.heart {
        let _ = writeln!(
            out,
            "heart, where its sky is read: {x:.1} {y:.1} {h:.1}, the middle of the chunk nearest the middle of its ground"
        );
    }
    let places: Vec<String> = z
        .places
        .iter()
        .map(|(name, n)| format!("{name} {}", thousands((*n).into())))
        .collect();
    let _ = writeln!(out, "places, by chunks: {}", places.join(", "));
    if z.heart.is_some() {
        let _ = writeln!(out, "sky: {}-sky.png", z.key);
        for line in sky {
            let _ = writeln!(out, "  {line}");
        }
    }
    let _ = writeln!(
        out,
        "water: {} (a share of its cells, each {:.2} yd square)",
        water(z),
        CELL_YARDS
    );
    let _ = writeln!(out, "music: {}", or_none(&sound.music));
    for line in &sound.music_lines {
        let _ = writeln!(out, "  {line}");
    }
    let _ = writeln!(out, "ambience: {}", or_none(&sound.ambience));
    for line in &sound.ambience_lines {
        let _ = writeln!(out, "  {line}");
    }
    let d = &z.doodads;
    let census = [
        (d.trees, "trees"),
        (d.shrubs, "shrubs"),
        (d.rocks, "rocks"),
        (d.fences, "fences"),
        (d.props, "props"),
    ]
    .map(|(n, kind)| format!("{} {kind}", thousands(n as u64)));
    let _ = writeln!(
        out,
        "doodads standing on its ground, each once: {}; {} buildings",
        census.join(", "),
        thousands(z.buildings.into())
    );
    let _ = writeln!(out, "ground textures, as a share of its ground:");
    for &(g, t) in &z.grounds {
        let ground = &inv.grounds[g];
        let _ = writeln!(
            out,
            "  {} {} ({})",
            share(t, zone_texels(z)),
            ground.path,
            ground.kind
        );
    }
    let _ = writeln!(out, "models placed in it, most first:");
    for &t in &z.models {
        let model = &inv.models[t.index];
        let _ = writeln!(
            out,
            "  {} {} ({}; {})",
            placed(t),
            model.path,
            model.kind,
            size(model.bounds)
        );
    }
    out
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "none" } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_names_read_as_written() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(stem("World\\A\\LampPost.mdx"), "LampPost");
        assert_eq!(share(1.0, 3.0), "33%");
        assert_eq!(share(1.0, 300.0), "0.3%");
        assert_eq!(share(1.0, 3000.0), "under 0.1%");
        assert_eq!(
            size(Some([[-0.2, -1.1, 0.0], [0.2, 1.2, 4.1]])),
            "4.1 yd tall, 0.4 x 2.3 across"
        );
    }
}
