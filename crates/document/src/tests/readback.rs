#![allow(clippy::float_cmp)]

use std::collections::BTreeMap;
use std::io::Cursor;

use super::{FakeInstall, SCRIPT, journal, make, scratch};
use crate::build::{Built, mmdx_name};
use crate::frame::{CELL, CENTRE, CHUNK, TILE, heading_of};
use crate::zone::{TEXELS_ACROSS, Zone};

fn nibble(data: &[u8], at: usize, t: usize) -> f32 {
    let (r, c) = ((t / TEXELS_ACROSS).min(62), (t % TEXELS_ACROSS).min(62));
    let k = r * TEXELS_ACROSS + c;
    let byte = data.get(at + k / 2).copied().unwrap_or(0);
    f32::from(if k.is_multiple_of(2) {
        byte & 15
    } else {
        byte >> 4
    }) / 15.0
}

#[allow(clippy::too_many_lines)]
fn differences(z: &Zone, b: &Built) -> Vec<String> {
    let mut out = Vec::new();
    let mut doodads = BTreeMap::new();
    let mut wmos = BTreeMap::new();
    let east = z.chunks_east();
    for (&(x, y), bytes) in &b.tiles {
        let (tx, ty) = (
            (x - z.frame.origin.0) as usize,
            (y - z.frame.origin.1) as usize,
        );
        let Ok(adt) = adt::parse_adt(bytes) else {
            out.push(format!("tile {x},{y} does not read"));
            continue;
        };
        for (e, c) in adt.mcnk_chunks.iter().enumerate() {
            let (gx, gy) = (tx * 16 + e % 16, ty * 16 + e / 16);
            let h = &c.header;
            let corner = [
                CENTRE - (f64::from(z.frame.origin.1) * TILE + gy as f64 * CHUNK),
                CENTRE - (f64::from(z.frame.origin.0) * TILE + gx as f64 * CHUNK),
            ];
            if (f64::from(h.position[0]) - corner[0]).abs() > 0.01
                || (f64::from(h.position[1]) - corner[1]).abs() > 0.01
            {
                out.push(format!("chunk {gx},{gy} stands at {:?}", h.position));
            }
            let heights = c.heights.as_ref().map_or(&[][..], |m| &m.heights[..]);
            for (i, v) in heights.iter().enumerate() {
                let (r, cc) = (i / 17, i % 17);
                let want = if cc < 9 {
                    z.heights.outer(gx * 8 + cc, gy * 8 + r)
                } else {
                    z.heights.inner(gx * 8 + cc - 9, gy * 8 + r)
                };
                if (h.position[2] + v - want).abs() > 1e-3 {
                    out.push(format!(
                        "chunk {gx},{gy} vertex {i}: {} for {want}",
                        h.position[2] + v
                    ));
                    break;
                }
            }
            if h.area_id != z.settings.borrow.as_ref().map_or(0, |b| b.area) {
                out.push(format!("chunk {gx},{gy} in area {}", h.area_id));
            }
            let paint = &z.paint[gy * east + gx];
            let layers = c.layers.as_ref().map_or(&[][..], |l| &l.layers[..]);
            let named: Vec<(String, u32)> = layers
                .iter()
                .map(|l| {
                    let t = adt
                        .textures
                        .get(l.texture_id as usize)
                        .cloned()
                        .unwrap_or_default();
                    (t, l.effect_id)
                })
                .collect();
            let want: Vec<(String, u32)> = paint
                .layers
                .iter()
                .map(|l| {
                    let t = &z.palette[usize::from(l.palette_place)];
                    (t.path.clone(), t.effect)
                })
                .collect();
            if named != want {
                out.push(format!("chunk {gx},{gy} layers {named:?} for {want:?}"));
                continue;
            }
            let data = c.alpha.as_ref().map_or(&[][..], |a| &a.data[..]);
            let mut worst = 0f32;
            for t in 0..TEXELS_ACROSS * TEXELS_ACROSS {
                let mut rest = 1.0;
                for (l, layer) in layers.iter().enumerate().rev() {
                    let a = if l == 0 {
                        1.0
                    } else {
                        nibble(data, layer.offset_in_mcal as usize, t)
                    };
                    let shown = rest * a;
                    rest *= 1.0 - a;
                    let tt =
                        (t / TEXELS_ACROSS).min(62) * TEXELS_ACROSS + (t % TEXELS_ACROSS).min(62);
                    let w = f32::from(paint.layers[l].w[tt]) / 255.0;
                    worst = worst.max((shown - w).abs());
                }
            }
            if worst > 0.09 {
                out.push(format!("chunk {gx},{gy} shows a weight {worst} off"));
            }
            let wet: Vec<bool> = c.liquids.first().map_or(vec![false; 64], |l| {
                l.tile_flags.iter().map(|f| f & 15 != 15).collect()
            });
            for (k, &is) in wet.iter().enumerate() {
                let (r, cc) = (k / 8, k % 8);
                let middle = [
                    (gx as f64 * 8.0 + cc as f64 + 0.5) * CELL,
                    (gy as f64 * 8.0 + r as f64 + 0.5) * CELL,
                ];
                let low = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .map(|&(a, b)| f64::from(z.heights.outer(gx * 8 + cc + a, gy * 8 + r + b)))
                    .fold(f64::MAX, f64::min);
                let level = z
                    .water
                    .values()
                    .rfind(|w| w.shape.reach(middle).is_some() && low < w.level as f64 / 100.0)
                    .map(|w| w.level as f64 / 100.0);
                if level.is_some() != is {
                    out.push(format!(
                        "chunk {gx},{gy} cell {k} wet {is}, want {}",
                        level.is_some()
                    ));
                } else if let (Some(level), Some(l)) = (level, c.liquids.first()) {
                    let v = &l.vertices[r * 9 + cc];
                    if (f64::from(v.height) - level).abs() > 1e-3 {
                        out.push(format!("chunk {gx},{gy} water at {} for {level}", v.height));
                    }
                }
            }
        }
        for d in &adt.doodad_placements {
            let model = adt
                .models
                .get(d.name_id as usize)
                .cloned()
                .unwrap_or_default();
            doodads.insert(d.unique_id, (model, d.position, d.rotation, d.scale));
        }
        for w in &adt.wmo_placements {
            let model = adt
                .wmos
                .get(w.name_id as usize)
                .cloned()
                .unwrap_or_default();
            wmos.insert(w.unique_id, (model, w.position, w.rotation, w.doodad_set));
        }
        let Ok(mesh) = terrain::adt_to_tile_mesh(bytes) else {
            out.push(format!("tile {x},{y} does not mesh"));
            continue;
        };
        for k in 0..97u32 {
            let p = [
                (tx as f64 + f64::from(k % 11) / 10.0 * 0.999) * TILE,
                (ty as f64 + f64::from(k / 11) / 10.0 * 0.999) * TILE,
            ];
            let w = z.frame.world(p);
            let got = terrain::terrain_height_at(&mesh.chunks, [w[0] as f32, w[1] as f32, 0.0]);
            let want = z.heights.ground(p);
            if got.is_none_or(|g| (f64::from(g) - want).abs() > 0.01) {
                out.push(format!("the ground at {p:?} is {got:?}, not {want}"));
            }
        }
    }
    for (id, t) in &z.things {
        let unique = z.unique_id(id).unwrap_or_default();
        let p = t.at();
        let position = [
            (f64::from(z.frame.origin.0) * TILE + p[0]) as f32,
            t.world_z(&z.heights) as f32,
            (f64::from(z.frame.origin.1) * TILE + p[1]) as f32,
        ];
        let heading = heading_of(t.facing_deg()) as f32;
        let stands = |rot: [f32; 3]| {
            if !t.lean {
                return rot[0] == 0.0 && rot[2] == 0.0;
            }
            let [east, south] = z.heights.rise(p);
            let len = (east * east + south * south + 1.0).sqrt();
            let up = client_up(rot);
            (up[0] - south / len).abs() < 1e-4
                && (up[1] - east / len).abs() < 1e-4
                && (up[2] - 1.0 / len).abs() < 1e-4
        };
        let found = match t.set {
            None => doodads.remove(&unique).map(|(m, pos, rot, scale)| {
                m.eq_ignore_ascii_case(&mmdx_name(&t.model))
                    && pos == position
                    && rot[1] == heading
                    && stands(rot)
                    && scale == t.scale
            }),
            Some(set) => wmos.remove(&unique).map(|(m, pos, rot, s)| {
                m.eq_ignore_ascii_case(&t.model) && pos == position && rot[1] == heading && s == set
            }),
        };
        if found != Some(true) {
            out.push(format!("{id} is {found:?} in the tiles"));
        }
    }
    if !doodads.is_empty() || !wmos.is_empty() {
        out.push(format!(
            "{} placements the zone lacks",
            doodads.len() + wmos.len()
        ));
    }
    let wdt = wdt::WdtReader::new(Cursor::new(b.wdt.clone())).read();
    let wdl = wdl::WdlFile::parse(&b.wdl);
    for x in 0..64u32 {
        for y in 0..64u32 {
            let want = b.tiles.contains_key(&(x, y));
            let in_wdt = wdt.as_ref().is_ok_and(|w| {
                w.get_tile(x as usize, y as usize)
                    .is_some_and(|t| t.has_adt)
            });
            let in_wdl = wdl.as_ref().is_ok_and(|w| w.is_present(x, y));
            if (in_wdt, in_wdl) != (want, want) {
                out.push(format!(
                    "tile {x},{y}: WDT {in_wdt}, WDL {in_wdl}, zone {want}"
                ));
            }
        }
    }
    out
}

fn client_up(rot: [f32; 3]) -> [f64; 3] {
    let [rx, ry, rz] = rot.map(|d| f64::from(d).to_radians());
    let about_x = |a: f64, v: [f64; 3]| {
        let (s, c) = (a.sin(), a.cos());
        [v[0], c * v[1] - s * v[2], s * v[1] + c * v[2]]
    };
    let about_y = |a: f64, v: [f64; 3]| {
        let (s, c) = (a.sin(), a.cos());
        [c * v[0] + s * v[2], v[1], -s * v[0] + c * v[2]]
    };
    let about_z = |a: f64, v: [f64; 3]| {
        let (s, c) = (a.sin(), a.cos());
        [c * v[0] - s * v[1], s * v[0] + c * v[1], v[2]]
    };
    let quarter = std::f64::consts::FRAC_PI_2;
    about_x(
        quarter,
        about_y(
            ry - std::f64::consts::PI,
            about_z(-rx, about_x(rz - quarter, [0.0, 0.0, 1.0])),
        ),
    )
}

fn built() -> (Zone, Built) {
    let doc = make(&scratch("readback"), &journal("sam", SCRIPT));
    let b = doc
        .build(&mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    (doc.zone().clone(), b)
}

/// `MVER` is 12 bytes and `MHDR` 72; `MCIN`'s entries follow its own 8.
const MCIN_ENTRIES: usize = 12 + 72 + 8;
/// `MVER` is 12 bytes and `MPHD` 40; `MAIN`'s entries follow its own 8.
const MAIN_ENTRIES: usize = 12 + 40 + 8;

fn chunk_at(tile: &[u8], e: usize) -> usize {
    let entry = MCIN_ENTRIES + e * 16;
    u32::from_le_bytes([
        tile[entry],
        tile[entry + 1],
        tile[entry + 2],
        tile[entry + 3],
    ]) as usize
}

#[test]
fn the_files_read_back_through_cairns_readers() {
    let (z, b) = built();
    let found = differences(&z, &b);
    assert!(found.is_empty(), "{found:#?}");
    let tilted = z
        .things
        .values()
        .filter(|t| t.lean && z.heights.slope(t.at()) > 3.0)
        .count();
    assert!(tilted > 0, "something leans on sloping ground");
}

#[test]
fn the_controls_each_duty_undone_is_caught() {
    let (z, b) = built();
    let first = *b.tiles.keys().next().unwrap_or(&(0, 0));
    let base_off = |b: &mut Built| {
        let t = b.tiles.get_mut(&first).unwrap_or_else(|| panic!("a tile"));
        let at = chunk_at(t, 17) + 8 + 0x70;
        let v = f32::from_le_bytes([t[at], t[at + 1], t[at + 2], t[at + 3]]) + 1.0;
        t[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let no_tile = |b: &mut Built| {
        let at = MAIN_ENTRIES + (first.1 as usize * 64 + first.0 as usize) * 8;
        b.wdt[at] = 0;
    };
    let other_id = |b: &mut Built| {
        for t in b.tiles.values_mut() {
            let at = t.windows(4).position(|w| w == b"FDDM").unwrap_or(0);
            if t[at + 4..at + 8] != [0; 4] {
                t[at + 8 + 4] ^= 1;
            }
        }
    };
    let high = |b: &mut Built| {
        for t in b.tiles.values_mut() {
            for e in 0..256 {
                let header = chunk_at(t, e);
                let word = |o: usize| u32::from_le_bytes([t[o], t[o + 1], t[o + 2], t[o + 3]]);
                let (at, size) = (
                    header + word(header + 8 + 0x60) as usize,
                    word(header + 8 + 0x64),
                );
                for block in 0..(size as usize).saturating_sub(8) / 0x324 {
                    for i in 0..81 {
                        let v = at + 8 + block * 0x324 + 8 + i * 8 + 4;
                        let h = f32::from_le_bytes([t[v], t[v + 1], t[v + 2], t[v + 3]]) + 1.0;
                        t[v..v + 4].copy_from_slice(&h.to_le_bytes());
                    }
                }
            }
        }
    };
    let upright = |b: &mut Built| {
        for t in b.tiles.values_mut() {
            let at = t.windows(4).position(|w| w == b"FDDM").unwrap_or(0);
            let size = u32::from_le_bytes([t[at + 4], t[at + 5], t[at + 6], t[at + 7]]) as usize;
            for record in (at + 8..at + 8 + size).step_by(36) {
                for angle in [record + 20, record + 28] {
                    t[angle..angle + 4].copy_from_slice(&0f32.to_le_bytes());
                }
            }
        }
    };
    for (name, undo) in [
        ("a base height a yard off", &base_off as &dyn Fn(&mut Built)),
        ("a tile left out of the WDT", &no_tile),
        ("a placement's unique id changed", &other_id),
        ("the water a yard high", &high),
        ("every model stood upright", &upright),
    ] {
        let mut broken = Built {
            tiles: b.tiles.clone(),
            wdt: b.wdt.clone(),
            wdl: b.wdl.clone(),
            zone_file: b.zone_file.clone(),
        };
        undo(&mut broken);
        assert!(
            !differences(&z, &broken).is_empty(),
            "{name} was not caught"
        );
    }
}
