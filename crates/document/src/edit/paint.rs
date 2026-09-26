use std::fmt::Write as _;

use crate::command::Paint;
use crate::frame::{CHUNK, TEXEL};
use crate::image::Image;
use crate::mask::Masking;
use crate::shape::{Shape, falloff};
use crate::text::two_places;
use crate::zone::{ChunkPaint, MAX_LAYERS, PaintLayer, TEXELS_ACROSS, TEXELS_IN_CHUNK, Zone};

fn chunks_touched(z: &Zone, s: &Shape) -> Vec<(usize, usize)> {
    let (east, south) = z.frame.chunks();
    let [lo, hi] = s.bounds();
    let at = |v: f64, n: usize| ((v / CHUNK).floor().max(0.0) as usize).min(n - 1);
    let mut out = Vec::new();
    for gy in at(lo[1], south)..=at(hi[1], south) {
        for gx in at(lo[0], east)..=at(hi[0], east) {
            out.push((gx, gy));
        }
    }
    out
}

pub fn apply(z: &mut Zone, tex: u16, p: &Paint, masking: &Masking<'_>) -> (String, Image) {
    let east = z.chunks_east();
    let (mut touched, mut skipped, mut dropped) = (0usize, Vec::new(), Vec::new());
    let mut image = Image::default();
    for (gx, gy) in chunks_touched(z, &p.area) {
        let o = [gx as f64 * CHUNK, gy as f64 * CHUNK];
        let share: Vec<u32> = (0..TEXELS_IN_CHUNK)
            .map(|k| {
                let at = [
                    o[0] + ((k % TEXELS_ACROSS) as f64 + 0.5) * TEXEL,
                    o[1] + ((k / TEXELS_ACROSS) as f64 + 0.5) * TEXEL,
                ];
                p.area.reach(at).map_or(0, |r| {
                    let stroke = falloff(r.t, p.falloff) * p.strength;
                    let lets = if stroke > 0.0 {
                        masking.weight(z, at, p.slope.as_ref())
                    } else {
                        0.0
                    };
                    (stroke * lets * 255.0).round().clamp(0.0, 255.0) as u32
                })
            })
            .collect();
        if share.iter().all(|&v| v == 0) {
            continue;
        }
        let chunk = gy * east + gx;
        let was = z.paint[chunk].clone();
        let c = &mut z.paint[chunk];
        let layer = if let Some(i) = c.layers.iter().position(|l| l.palette_place == tex) {
            i
        } else {
            if c.layers.len() >= MAX_LAYERS {
                if !p.make_room {
                    skipped.push((gx, gy));
                    continue;
                }
                dropped.push((gx, gy, drop_weakest(c, tex)));
            }
            c.layers.push(PaintLayer {
                palette_place: tex,
                w: vec![0; TEXELS_IN_CHUNK],
            });
            c.layers.len() - 1
        };
        for (k, &s) in share.iter().enumerate() {
            if s == 0 {
                continue;
            }
            let mut rest = 0u32;
            for (i, l) in c.layers.iter_mut().enumerate() {
                if i != layer {
                    l.w[k] = ((u32::from(l.w[k]) * (255 - s) + 127) / 255) as u8;
                    rest += u32::from(l.w[k]);
                }
            }
            c.layers[layer].w[k] = (255 - rest) as u8;
        }
        drop_painted_out(c);
        touched += 1;
        if let Some(ci) = Image::changed_paint(chunk, &was, &z.paint[chunk]) {
            image.paint.push(ci);
        }
    }
    let mut reply = format!("{touched} chunks painted");
    if !skipped.is_empty() {
        let _ = write!(
            reply,
            "; {} chunks left alone, each already holding four textures (the format's most; \
             `--make-room` drops each one's least-used other texture): {}",
            skipped.len(),
            list_chunks(&skipped)
        );
    }
    for (gx, gy, d) in &dropped {
        let _ = write!(
            reply,
            "; chunk {gx},{gy} dropped {} (it covered {} % of the chunk)",
            z.palette[usize::from(d.palette_place)].path,
            two_places(d.covered_percent)
        );
    }
    (reply, image)
}

fn list_chunks(v: &[(usize, usize)]) -> String {
    let named: Vec<String> = v
        .iter()
        .take(12)
        .map(|&(x, y)| {
            format!(
                "{x},{y} (x {}..{}, y {}..{})",
                two_places(x as f64 * CHUNK),
                two_places((x + 1) as f64 * CHUNK),
                two_places(y as f64 * CHUNK),
                two_places((y + 1) as f64 * CHUNK)
            )
        })
        .collect();
    let more = if v.len() > 12 {
        format!(" and {} more", v.len() - 12)
    } else {
        String::new()
    };
    format!("{}{more}", named.join(", "))
}

struct Dropped {
    palette_place: u16,
    covered_percent: f64,
}

fn drop_weakest(c: &mut ChunkPaint, keep: u16) -> Dropped {
    let total = |l: &PaintLayer| l.w.iter().map(|&v| u64::from(v)).sum::<u64>();
    let weakest = c
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.palette_place != keep)
        .min_by_key(|(i, l)| (total(l), std::cmp::Reverse(*i)))
        .map_or(0, |(i, _)| i);
    let gone = c.layers.remove(weakest);
    let covered_percent = total(&gone) as f64 / (TEXELS_IN_CHUNK as f64 * 255.0) * 100.0;
    for k in 0..TEXELS_IN_CHUNK {
        let d = u32::from(gone.w[k]);
        if d == 0 {
            continue;
        }
        let sum: u32 = c.layers.iter().map(|l| u32::from(l.w[k])).sum();
        if sum == 0 {
            c.layers[0].w[k] = d as u8;
            continue;
        }
        let (mut given, n) = (0, c.layers.len());
        for i in 0..n {
            let add = if i + 1 == n {
                d - given
            } else {
                u32::from(c.layers[i].w[k]) * d / sum
            };
            c.layers[i].w[k] = (u32::from(c.layers[i].w[k]) + add) as u8;
            given += add;
        }
    }
    Dropped {
        palette_place: gone.palette_place,
        covered_percent,
    }
}

fn drop_painted_out(c: &mut ChunkPaint) {
    while c.layers.len() > 1 {
        let Some(i) = c.layers.iter().position(|l| l.w.iter().all(|&v| v == 0)) else {
            break;
        };
        c.layers.remove(i);
    }
}
