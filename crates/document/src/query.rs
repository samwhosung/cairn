use std::fmt::Write as _;

use crate::frame::{CHUNK, TEXEL, bearing_of, bearing_vec, compass};
use crate::install::{Install, ModelBox};
use crate::shape::Shape;
use crate::text::{from_centi, scale_text, two_places};
use crate::zone::{Id, TEXELS_ACROSS, Thing, Zone};

fn kind(t: &Thing) -> &'static str {
    if t.is_building() {
        return "building";
    }
    let name = t
        .model
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&t.model)
        .to_ascii_lowercase();
    let any = |w: &[&str]| w.iter().any(|k| name.contains(k));
    if any(&["tree", "canopy", "palm", "trunk", "log"]) {
        "tree"
    } else if any(&[
        "bush", "shrub", "plant", "fern", "flower", "grass", "weed", "reed", "vine", "root",
        "mushroom", "cactus",
    ]) {
        "bush"
    } else if any(&["rock", "stone", "boulder", "cliff", "pebble"]) {
        "rock"
    } else if any(&["fence", "post", "wall", "rail", "gate"]) {
        "fence"
    } else {
        "prop"
    }
}

fn on_ground(t: &Thing, b: &ModelBox) -> Shape {
    let s = t.scale_f();
    let front = bearing_vec(t.facing_deg());
    let left = bearing_vec(t.facing_deg() - 90.0);
    let p = t.at();
    let c = |mx: f32, my: f32| {
        let (mx, my) = (f64::from(mx) * s, f64::from(my) * s);
        [
            p[0] + mx * front[0] + my * left[0],
            p[1] + mx * front[1] + my * left[1],
        ]
    };
    Shape::Poly {
        pts: vec![
            c(b.min[0], b.min[1]),
            c(b.max[0], b.min[1]),
            c(b.max[0], b.max[1]),
            c(b.min[0], b.max[1]),
        ],
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

pub fn thing_line(
    z: &Zone,
    install: &mut dyn Install,
    id: &Id,
    t: &Thing,
    from: Option<[f64; 2]>,
) -> String {
    let p = t.at();
    let size = install.model_box(&t.model).map_or("?".into(), |b| {
        let (s, k) = (b.size(), t.scale_f());
        format!(
            "{}×{}×{} yd",
            two_places(f64::from(s[0]) * k),
            two_places(f64::from(s[1]) * k),
            two_places(f64::from(s[2]) * k)
        )
    });
    let rel = from.map_or(String::new(), |f| {
        let d = dist(p, f);
        if d < 0.05 {
            "here  ".into()
        } else {
            format!(
                "{} yd {}  ",
                two_places(d),
                compass(bearing_of([p[0] - f[0], p[1] - f[1]]))
            )
        }
    });
    let set = t.set.map_or(String::new(), |s| format!(" set {s}"));
    format!(
        "{id}  {rel}at {},{} z {}  facing {} ({})  scale {}{set}  {size}  {}  {}",
        from_centi(t.x),
        from_centi(t.y),
        two_places(t.world_z(&z.heights)),
        from_centi(t.facing),
        compass(t.facing_deg()),
        scale_text(t.scale),
        kind(t),
        t.model
    )
}

pub fn things(
    z: &Zone,
    install: &mut dyn Install,
    near: Option<([f64; 2], f64)>,
    words: Option<&str>,
) -> String {
    let words = words.map(str::to_ascii_lowercase);
    let mut s = String::new();
    let mut n = 0;
    for (id, t) in &z.things {
        if near.is_some_and(|(p, r)| dist(t.at(), p) > r) {
            continue;
        }
        if words
            .as_ref()
            .is_some_and(|w| !t.model.to_ascii_lowercase().contains(w))
        {
            continue;
        }
        let _ = writeln!(s, "{}", thing_line(z, install, id, t, near.map(|(p, _)| p)));
        n += 1;
    }
    let _ = writeln!(s, "{n} things");
    s
}

fn paint_at(z: &Zone, p: [f64; 2], s: &mut String) {
    let (east, south) = z.frame.chunks();
    let gx = ((p[0] / CHUNK) as usize).min(east - 1);
    let gy = ((p[1] / CHUNK) as usize).min(south - 1);
    let c = &z.paint[gy * east + gx];
    let tx = (((p[0] - gx as f64 * CHUNK) / TEXEL) as usize).min(TEXELS_ACROSS - 1);
    let ty = (((p[1] - gy as f64 * CHUNK) / TEXEL) as usize).min(TEXELS_ACROSS - 1);
    let path = |tex: u16| z.palette[usize::from(tex)].path.as_str();
    let short = |tex: u16| path(tex).rsplit(['\\', '/']).next().unwrap_or_default();
    let paint: Vec<String> = c
        .layers
        .iter()
        .filter(|l| l.w[ty * TEXELS_ACROSS + tx] > 0)
        .map(|l| {
            format!(
                "{} {} %",
                path(l.palette_place),
                two_places(f64::from(l.w[ty * TEXELS_ACROSS + tx]) / 2.55)
            )
        })
        .collect();
    let _ = writeln!(
        s,
        "paint: {} (chunk {gx},{gy} holds {} of the 4 textures a chunk can: {})",
        paint.join(", "),
        c.layers.len(),
        c.layers
            .iter()
            .map(|l| short(l.palette_place))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn water_at(z: &Zone, p: [f64; 2], ground: f64, s: &mut String) {
    let mut wet = false;
    for (id, w) in &z.water {
        if w.shape.reach(p).is_none() {
            continue;
        }
        let level = w.level as f64 / 100.0;
        if level > ground {
            let _ = writeln!(
                s,
                "water {id} at {} yd: {} yd deep here",
                two_places(level),
                two_places(level - ground)
            );
        } else {
            let _ = writeln!(
                s,
                "water {id} at {} yd: the ground here is {} yd above it",
                two_places(level),
                two_places(ground - level)
            );
        }
        wet = true;
    }
    if !wet {
        s.push_str("water: none\n");
    }
}

pub fn here(z: &Zone, install: &mut dyn Install, p: [f64; 2], r: f64) -> Result<String, String> {
    if !z.frame.contains(p) {
        return Err(format!(
            "{},{} is off the zone",
            two_places(p[0]),
            two_places(p[1])
        ));
    }
    let mut s = String::new();
    let ground = z.heights.ground(p);
    let _ = writeln!(
        s,
        "at {},{}: ground {} yd, slope {}°",
        two_places(p[0]),
        two_places(p[1]),
        two_places(ground),
        two_places(z.heights.slope(p))
    );
    paint_at(z, p, &mut s);
    water_at(z, p, ground, &mut s);
    let mut near: Vec<(f64, &Id)> = z
        .things
        .iter()
        .filter_map(|(id, t)| {
            let d = dist(t.at(), p);
            (d <= r).then_some((d, id))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)));
    let _ = writeln!(s, "within {} yd: {}", two_places(r), near.len());
    for (_, id) in near.iter().take(40) {
        let _ = writeln!(
            s,
            "  {}",
            thing_line(z, install, id, &z.things[*id], Some(p))
        );
    }
    for (id, t) in &z.things {
        if near.iter().any(|n| n.1 == id) {
            continue;
        }
        if let Ok(b) = install.model_box(&t.model)
            && on_ground(t, &b).reach(p).is_some()
        {
            let _ = writeln!(s, "  over it: {}", thing_line(z, install, id, t, Some(p)));
        }
    }
    Ok(s)
}
