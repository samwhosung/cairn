use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use fits::{AROUND, Evidence, HEADER, Own, Spot, Tables, Tally};

use super::Borrows;

const ORDERS: [&str; 7] = [
    "shuffled (the control)",
    "most placed first",
    "the borrowed zone's palette",
    "what stood within 20 yd",
    "the palette and what stood within 20 yd",
    "the lists: palette, the zone's own, beside",
    "the zone's own placements alone",
];

/// Ranks every model before each placement a zone's history makes, from what the zone held then,
/// and tallies where each list order put the model placed.
pub(super) fn run(
    tables: &Tables,
    file: &Path,
    borrows: Option<&Borrows>,
) -> Result<String, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let palette = match borrows {
        None => return Err("a replay needs --borrows ZONE, or --borrows none".into()),
        Some(Borrows::Nothing) => None,
        Some(Borrows::Zone(name)) => Some(
            tables
                .zone_named(name)
                .ok_or_else(|| format!("the catalog has no zone named {name}"))?,
        ),
    };
    let nothing = Own::default();
    let install = Evidence::new(tables, &nothing, true);
    let everywhere = install.everywhere();
    let borrowed = install.palette(palette);
    let control = fits::shuffled(tables);
    let mut tallies: Vec<Tally> = ORDERS.iter().map(|n| Tally::new(n)).collect();
    let mut own = Own::default();
    let (mut placed, mut moved, mut removed, mut unknown) = (0, 0, 0, 0);
    let mut ranking = Duration::ZERO;
    for (i, line) in text.lines().enumerate() {
        let at_line = |e: String| format!("{}:{}: {e}", file.display(), i + 1);
        let line = line.split('#').next().unwrap_or_default().trim();
        let mut words = line.split_whitespace();
        let (Some(verb), Some(id)) = (words.next(), words.next()) else {
            if line.is_empty() {
                continue;
            }
            return Err(at_line(
                "want `place ID X,Y PATH`, `move ID X,Y` or `remove ID ...`".into(),
            ));
        };
        match verb {
            "place" => {
                let at = super::point(words.next().unwrap_or_default())
                    .map_err(|e| at_line(format!("want {e}")))?;
                let path = words.collect::<Vec<_>>().join(" ");
                placed += 1;
                let Some(m) = tables.model(&path) else {
                    unknown += 1;
                    continue;
                };
                let spot = Spot {
                    zone: palette,
                    ground: None,
                    near: own.around(at),
                };
                let near = install.beside(AROUND, &spot.beside(AROUND));
                let started = Instant::now();
                let lists = Evidence::new(tables, &own, palette.is_some()).scores(&spot);
                ranking += started.elapsed();
                let alone = Spot {
                    zone: None,
                    ..spot.clone()
                };
                let alone = Evidence::new(tables, &own, false).scores(&alone);
                let orders = [
                    control.clone(),
                    everywhere.clone(),
                    borrowed.clone(),
                    sum(&everywhere, &near),
                    sum(&borrowed, &near),
                    lists,
                    alone,
                ];
                for (tally, score) in tallies.iter_mut().zip(&orders) {
                    tally.add(tables, score, m);
                }
                own.place(id, m, at, None);
            }
            "move" => {
                let at = super::point(words.next().unwrap_or_default())
                    .map_err(|e| at_line(format!("want {e}")))?;
                if let Some((m, _)) = own.get(id) {
                    own.place(id, m, at, None);
                    moved += 1;
                }
            }
            "remove" => {
                for id in std::iter::once(id).chain(words) {
                    removed += usize::from(own.remove(id));
                }
            }
            _ => return Err(at_line(format!("no verb {verb}"))),
        }
    }
    let ranked = tallies[0].len();
    let mut out = format!(
        "cairn: {placed} placed, {moved} moved and {removed} removed; each of the {ranked} of a \
         model the catalog knows ranked among its kind from what stood before it, {unknown} left \
         out\n{HEADER}\n"
    );
    for t in &tallies {
        let _ = writeln!(out, "{}", t.row());
    }
    let _ = write!(
        out,
        "cairn: a list took {:.2} ms on average",
        ranking.as_secs_f64() * 1000.0 / ranked.max(1) as f64
    );
    Ok(out)
}

fn sum(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}
