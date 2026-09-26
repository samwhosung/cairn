use std::path::PathBuf;

use super::{journal, scratch};
use crate::document::Document;
use crate::install::Archives;
use crate::relief::{self, Ground};
use crate::zone::Heights;

const BANDS: [f64; 5] = [9.0, 18.0, 27.0, 36.0, 45.0];
const RELIEFS: [&str; 3] = ["Elwynn Forest", "Redridge Mountains", "Alterac Mountains"];
const OCTAVES: [(f64, f64); 4] = [(50.0, 1.0), (25.0, 0.5), (12.5, 0.25), (6.25, 0.125)];

fn slope_mix(
    cols: usize,
    rows: usize,
    corner: impl Fn(usize, usize) -> f64,
    known: impl Fn(usize, usize) -> bool,
) -> [f64; 6] {
    let cell = crate::frame::CELL;
    let mut n = [0.0; 6];
    let mut all = 0.0;
    for j in 0..rows {
        for i in 0..cols {
            if !known(i, j) {
                continue;
            }
            let south = (corner(i, j + 1) + corner(i + 1, j + 1) - corner(i, j) - corner(i + 1, j))
                / (2.0 * cell);
            let east = (corner(i + 1, j) + corner(i + 1, j + 1) - corner(i, j) - corner(i, j + 1))
                / (2.0 * cell);
            let slope = libm::atan(east.hypot(south)).to_degrees();
            n[BANDS.iter().take_while(|&&b| slope >= b).count()] += 1.0;
            all += 1.0;
        }
    }
    n.map(|v| v / all)
}

fn own_mix(g: &Ground) -> [f64; 6] {
    let h = &g.heights;
    slope_mix(
        h.cols,
        h.rows,
        |i, j| f64::from(h.outer(i, j)),
        |i, j| g.known[j * h.cols + i],
    )
}

fn zone_mix(h: &Heights) -> [f64; 6] {
    slope_mix(h.cols, h.rows, |i, j| f64::from(h.outer(i, j)), |_, _| true)
}

fn spread(h: &Heights, flat: f64) -> f64 {
    let s: f64 = h.outer.iter().map(|v| (f64::from(*v) - flat).powi(2)).sum();
    (s / h.outer.len() as f64).sqrt()
}

fn flat_with(lines: &[String], install: &mut Archives) -> Document {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    Document::replay(&lines, &scratch("relief"), install).unwrap_or_else(|e| panic!("{e}"))
}

fn row(name: &str, m: [f64; 6]) -> String {
    let shares: Vec<String> = m.iter().map(|v| format!("{:.0} %", v * 100.0)).collect();
    format!("| {name} | {} |", shares.join(" | "))
}

fn mix_distance(a: [f64; 6], b: [f64; 6]) -> f64 {
    a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f64>() / 2.0
}

#[test]
fn a_relief_keeps_its_zones_slopes_nearer_than_noise_of_its_spread() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let mut install = Archives::at(Some(data.clone()));
    let new = "new --tiles 1x1 --height 40 --texture Tileset\\Elwynn\\ElwynnGrassBase.blp --borrow Elwynn Forest --name Flat";
    println!(
        "| ground | 0-9° | 9-18° | 18-27° | 27-36° | 36-45° | 45°+ |\n|---|---:|---:|---:|---:|---:|---:|"
    );
    for name in RELIEFS {
        let chain = mpq::Chain::open(&data).unwrap_or_else(|e| panic!("{e}"));
        let areas = {
            let bytes = chain
                .read("DBFilesClient\\AreaTable.dbc")
                .unwrap_or_else(|e| panic!("{e}"));
            crate::install::Areas::parse(&bytes).unwrap_or_else(|e| panic!("{e}"))
        };
        let ground = relief::read(&chain, &areas, name).unwrap_or_else(|e| panic!("{e}"));
        let own = own_mix(&ground);
        let source = relief::Source::of(name, &ground).unwrap_or_else(|e| panic!("{e}"));
        let detail = Ground {
            heights: source.relief.clone(),
            known: ground.known.clone(),
        };
        let laid = flat_with(
            &journal(
                "ai",
                &[new, &format!("relief {name} --rect 0,0 533.34,533.34")],
            ),
            &mut install,
        );
        let laid_mix = zone_mix(&laid.zone().heights);
        let target = spread(&laid.zone().heights, 40.0);
        let noise = |scale: f64| -> Vec<String> {
            let mut lines = vec![new.to_owned()];
            for (k, (size, share)) in OCTAVES.iter().enumerate() {
                lines.push(format!(
                    "roughen {} --size {size} --at 266.67,266.67 --radius 400 --falloff 0 --seed {}",
                    scale * share,
                    k + 1
                ));
            }
            journal("ai", &lines.iter().map(String::as_str).collect::<Vec<_>>())
        };
        let unit = spread(&flat_with(&noise(1.0), &mut install).zone().heights, 40.0);
        let noisy = flat_with(&noise(target / unit), &mut install);
        let noise_mix = zone_mix(&noisy.zone().heights);
        let noise_spread = spread(&noisy.zone().heights, 40.0);
        println!("{}", row(&format!("{name}, its own ground"), own));
        println!(
            "{}",
            row(
                &format!(
                    "its own relief, on flat ground ({:.1} yd RMS)",
                    source.spread
                ),
                own_mix(&detail)
            )
        );
        println!(
            "{}",
            row(
                &format!("its relief on a flat tile ({target:.1} yd RMS)"),
                laid_mix
            )
        );
        println!(
            "{}",
            row(
                &format!("noise of that spread ({noise_spread:.1} yd RMS)"),
                noise_mix
            )
        );
        println!(
            "|  apart from its own: the relief {:.2}, the noise {:.2} |",
            mix_distance(own, laid_mix),
            mix_distance(own, noise_mix)
        );
        assert!(
            mix_distance(own, laid_mix) < mix_distance(own, noise_mix),
            "{name}: the relief's slopes lie nearer its own than noise's"
        );
    }
}
