//! The catalog's text: an index a row a thing for each of models, ground and zones, and a file of
//! detail for each thing.

use std::fmt::Write as _;

use atlas::Kind;

use crate::{Model, Survey, WATERS, Zone};

/// Yards a chunk's side.
const CHUNK_YARDS: f64 = 100.0 / 3.0;
const TEXELS_PER_CHUNK: f64 = 4096.0;
const CELL_YARDS: f64 = CHUNK_YARDS / 8.0;
/// A zone's ground or models listed in its row of the index, most first.
const ROW_TOP: usize = 5;
/// Painted-beside grounds listed for a texture.
const BESIDE_TOP: usize = 8;
/// A ground painted beside another in less of its ground than this isn't listed as beside it.
const BESIDE_LEAST: f64 = 0.01;

/// `1234567` as `1,234,567`.
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

/// The stem of a path's file name.
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

/// `4.1 yd tall, 0.4 x 2.3 across`.
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

fn scales(m: &Model) -> String {
    match m.scales {
        None => String::new(),
        Some([lo, _, _, _, hi]) if (hi - lo).abs() < 0.005 => format!("{lo:.2}"),
        Some([lo, _, _, _, hi]) => format!("{lo:.2} to {hi:.2}"),
    }
}

fn placed(on_ground: u32, in_buildings: u32) -> String {
    match (on_ground, in_buildings) {
        (g, 0) => thousands(g.into()),
        (0, i) => format!("{} inside buildings", thousands(i.into())),
        (g, i) => format!(
            "{} ({} inside buildings)",
            thousands((g + i).into()),
            thousands(i.into())
        ),
    }
}

fn zone_label(inv: &Survey, z: usize) -> &str {
    &inv.zones[z].name
}

pub(crate) fn models_tsv(inv: &Survey) -> String {
    let mut out = String::from("kind\tpath\tsize\tplaced\tscales\tzones\tpicture\n");
    for m in &inv.models {
        let zones: Vec<String> = m
            .zones
            .iter()
            .map(|&(z, g, i)| format!("{} {}", zone_label(inv, z), placed(g, i)))
            .collect();
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            m.kind,
            m.path,
            size(m.bounds),
            placed(m.on_ground, m.in_buildings),
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

fn zone_texels(z: &Zone) -> f64 {
    f64::from(z.chunks) * TEXELS_PER_CHUNK
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
            .filter(|(_, s)| *s >= BESIDE_LEAST)
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
            .take(ROW_TOP)
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
            .take(ROW_TOP)
            .map(|&(m, g, i)| {
                format!(
                    "{} {}",
                    stem(&inv.models[m].path),
                    thousands((g + i).into())
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
    let wet: Vec<String> = WATERS
        .iter()
        .zip(z.water)
        .filter(|(_, n)| *n > 0)
        .map(|(kind, n)| format!("{kind} {}", share(f64::from(n), f64::from(z.chunks) * 64.0)))
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
    if let Some([lo, p10, mid, p90, hi]) = m.scales {
        let _ = writeln!(
            out,
            "scales: {lo:.2} to {hi:.2}; a tenth below {p10:.2}, half below {mid:.2}, a tenth above {p90:.2}"
        );
    }
    let _ = writeln!(out, "where:");
    for &(z, g, i) in &m.zones {
        let zone = &inv.zones[z];
        let places: Vec<String> = m
            .places
            .iter()
            .filter(|(pz, _, _)| *pz == z)
            .map(|(_, name, n)| format!("{name} {}", thousands((*n).into())))
            .collect();
        let _ = writeln!(
            out,
            "  {} ({}): {} — {}",
            zone.name,
            zone.map_directory,
            placed(g, i),
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
        thousands((ground.texels / TEXELS_PER_CHUNK).round() as u64),
        thousands((ground.texels / TEXELS_PER_CHUNK * CHUNK_YARDS * CHUNK_YARDS).round() as u64)
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
    for &(o, s) in ground.beside.iter().filter(|(_, s)| *s >= BESIDE_LEAST) {
        let other = &inv.grounds[o];
        let _ = writeln!(out, "  {} ({}) {:.0}%", other.path, other.kind, 100.0 * s);
    }
    out
}

/// What the lighting tables say of a zone's sky, by the hours it is drawn at.
pub struct SkyLines(pub Vec<String>);

pub(crate) fn zone_txt(inv: &Survey, z: &Zone, sound: &ZoneSound, sky: &SkyLines) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", z.name);
    let _ = writeln!(
        out,
        "map: {} (Map.dbc {}); AreaTable {}",
        z.map_directory, z.map, z.area
    );
    let yards = f64::from(z.chunks) * CHUNK_YARDS * CHUNK_YARDS;
    let tiles = z.tiles.map_or(String::new(), |[x0, x1, y0, y1]| {
        format!(" in tiles {x0}..={x1} by {y0}..={y1}")
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
        for line in &sky.0 {
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
    let census: Vec<String> = Kind::ALL
        .iter()
        .zip(z.doodads)
        .map(|(k, n)| format!("{} {}s", thousands(n.into()), k.name()))
        .collect();
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
    for &(m, g, i) in &z.models {
        let model = &inv.models[m];
        let _ = writeln!(
            out,
            "  {} {} ({}; {})",
            placed(g, i),
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
