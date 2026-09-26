use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

pub const FILE: &str = "rules.tsv";
const HEADER: &str = "model\tslope\tapart\tscale\tstands\tplaced";
const SLOPE_PERCENTILES: [usize; 2] = [1, 95];
const SCALE_PERCENTILES: [usize; 2] = [10, 90];
const APART_PERCENTILE: usize = 10;
const PLACED_TWICE_WITHIN: f32 = 0.05;
/// No rule keeps two things closer than this, in yards, and no scatter does.
pub const LEAST_APART: f64 = 0.5;
const UPRIGHT_WITHIN: f64 = 2.0;
const NEAREST_WITHIN: f32 = 200.0;
const GRID: f32 = 25.0;

/// How a model stands where the install places it on the ground: the rules a scatter of it keeps
/// unless it is told otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// The slopes under it, in whole degrees: its 1st to its 95th in every hundred, or 0..90 when
    /// no placement of it has a known slope.
    pub slope: [f64; 2],
    /// Yards to the nearest other of it on its map, to a tenth: its 10th in every hundred; its
    /// footprint's widest side at its usual scale when none stands within 200 yd of another; never
    /// under [`LEAST_APART`].
    pub apart: f64,
    /// Its scales, to a hundredth: its 10th to its 90th in every hundred.
    pub scale: [f64; 2],
    /// Fewer than half its placements stand within 2° of upright; a scatter tilts it with the
    /// ground.
    pub lean: bool,
    pub placed: u32,
}

/// Each doodad placed on a map's ground, by its install path, and the rules it keeps there.
pub fn of(survey: &survey::Survey) -> Vec<(String, Rules)> {
    let mut by_model: BTreeMap<usize, Vec<&survey::Placement>> = BTreeMap::new();
    for p in &survey.placements {
        if !survey.models[p.model].building {
            by_model.entry(p.model).or_default().push(p);
        }
    }
    by_model
        .into_iter()
        .map(|(m, placed)| {
            let model = &survey.models[m];
            let map_of = |p: &survey::Placement| survey.zones[p.zone].map;
            let footprint = model.bounds.map_or(LEAST_APART, |[lo, hi]| {
                f64::from((hi[0] - lo[0]).max(hi[1] - lo[1]) * model.scales.map_or(1.0, |s| s.p50))
            });
            let apart = percentile(nearest_of_its_own(&placed, map_of), APART_PERCENTILE)
                .map_or(footprint, f64::from)
                .max(LEAST_APART);
            let slopes: Vec<f32> = placed.iter().filter_map(|p| p.slope).collect();
            let [lo, hi] = SLOPE_PERCENTILES.map(|q| percentile(slopes.clone(), q));
            let scales: Vec<f32> = placed.iter().map(|p| p.scale).collect();
            let [small, large] = SCALE_PERCENTILES.map(|q| percentile(scales.clone(), q));
            let upright = placed
                .iter()
                .filter(|p| tilt(p.rotation) < UPRIGHT_WITHIN)
                .count();
            let rules = Rules {
                slope: [
                    lo.map_or(0.0, |d| f64::from(d).floor()),
                    hi.map_or(90.0, |d| f64::from(d).ceil().min(90.0)),
                ],
                apart: tenth(apart),
                scale: [small.unwrap_or(1.0), large.unwrap_or(1.0)].map(hundredth),
                lean: 2 * upright < placed.len(),
                placed: placed.len() as u32,
            };
            (model.path.clone(), rules)
        })
        .collect()
}

/// Degrees from upright of a placement a map turns by the angles it stores as `[about y, about the
/// up axis, about x]`, applied about x, then y, then the up axis, which leaves the tilt alone.
fn tilt(rotation: [f32; 3]) -> f64 {
    let [about_y, _, about_x] = rotation.map(|d| f64::from(d).to_radians());
    (about_y.cos() * about_x.cos())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

fn percentile(mut v: Vec<f32>, q: usize) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[(v.len() - 1) * q / 100])
}

fn tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn hundredth(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

fn nearest_of_its_own(
    placed: &[&survey::Placement],
    map_of: impl Fn(&survey::Placement) -> u32,
) -> Vec<f32> {
    let cell = |p: &survey::Placement| {
        (
            map_of(p),
            (p.position[0] / GRID).floor() as i32,
            (p.position[1] / GRID).floor() as i32,
        )
    };
    let mut grid: HashMap<(u32, i32, i32), Vec<[f32; 2]>> = HashMap::new();
    for p in placed {
        grid.entry(cell(p))
            .or_default()
            .push([p.position[0], p.position[1]]);
    }
    let mut out = Vec::new();
    for p in placed {
        let (map, cx, cy) = cell(p);
        let mut best = f32::MAX;
        for ring in 0..=(NEAREST_WITHIN / GRID) as i32 {
            if best <= (ring - 1).max(0) as f32 * GRID {
                break;
            }
            for dx in -ring..=ring {
                for dy in -ring..=ring {
                    if dx.abs().max(dy.abs()) != ring {
                        continue;
                    }
                    for q in grid.get(&(map, cx + dx, cy + dy)).into_iter().flatten() {
                        let d = (p.position[0] - q[0]).hypot(p.position[1] - q[1]);
                        if d > PLACED_TWICE_WITHIN && d <= NEAREST_WITHIN {
                            best = best.min(d);
                        }
                    }
                }
            }
        }
        if best < f32::MAX {
            out.push(best);
        }
    }
    out
}

/// Writes the rules into `dir` unless it has them. Says whether it wrote them.
pub fn write(dir: &Path, rules: impl FnOnce() -> Vec<(String, Rules)>) -> Result<bool, String> {
    let path = dir.join(FILE);
    if path.exists() {
        return Ok(false);
    }
    let mut text = format!("{HEADER}\n");
    for (model, r) in rules() {
        let _ = writeln!(
            text,
            "{model}\t{}..{}\t{}\t{}..{}\t{}\t{}",
            r.slope[0],
            r.slope[1],
            r.apart,
            r.scale[0],
            r.scale[1],
            if r.lean { "leaning" } else { "upright" },
            r.placed
        );
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    survey::write_atomically(&path, |part| {
        std::fs::write(part, &text).map_err(|e| format!("{}: {e}", part.display()))
    })?;
    Ok(true)
}

/// The rules of each model placed on the install's ground.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book(BTreeMap<String, Rules>);

impl Book {
    /// The rules of the model at an install path, matched as the survey keys paths: in any case,
    /// with `/` or `\`, and without its extension.
    pub fn of(&self, model: &str) -> Option<Rules> {
        self.0.get(&survey::key(model)).copied()
    }
}

/// The rules `write` wrote into `dir`.
pub fn read(dir: &Path) -> Result<Book, String> {
    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e} (`cairn catalog` writes the models' rules)",
            path.display()
        )
    })?;
    let mut lines = text.lines();
    if lines.next() != Some(HEADER) {
        return Err(format!("{}: not a table of rules", path.display()));
    }
    let mut out = BTreeMap::new();
    for (i, line) in lines.enumerate() {
        let at = |e: String| format!("{}:{}: {e}", path.display(), i + 2);
        let f: Vec<&str> = line.split('\t').collect();
        let [model, slope, apart, scale, stands, placed] = f[..] else {
            return Err(at("want six columns".into()));
        };
        let rules = Rules {
            slope: range(slope).map_err(at)?,
            apart: number(apart).map_err(at)?,
            scale: range(scale).map_err(at)?,
            lean: match stands {
                "leaning" => true,
                "upright" => false,
                s => return Err(at(format!("`{s}` is neither upright nor leaning"))),
            },
            placed: number(placed).map_err(at)?,
        };
        let sane = rules.slope[0] <= rules.slope[1]
            && rules.scale[0] <= rules.scale[1]
            && rules.apart >= LEAST_APART
            && rules.scale[0] > 0.0;
        if !sane {
            return Err(at("a rule out of its range".into()));
        }
        out.insert(survey::key(model), rules);
    }
    Ok(Book(out))
}

fn number<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("`{s}` is not a number"))
}

fn range(s: &str) -> Result<[f64; 2], String> {
    let (lo, hi) = s
        .split_once("..")
        .ok_or_else(|| format!("`{s}` is not LO..HI"))?;
    let (lo, hi) = (number::<f64>(lo)?, number::<f64>(hi)?);
    if lo.is_finite() && hi.is_finite() {
        Ok([lo, hi])
    } else {
        Err(format!("`{s}` is not LO..HI"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(model: usize, at: [f32; 2], slope: f32, rotation: [f32; 3]) -> survey::Placement {
        survey::Placement {
            model,
            zone: 0,
            position: [at[0], at[1], 0.0],
            rotation,
            scale: 1.0 + at[0] / 1000.0,
            ground: None,
            slope: Some(slope),
        }
    }

    #[test]
    fn a_tilt_is_its_pitch_and_roll_whatever_its_heading() {
        assert!(tilt([0.0, 123.0, 0.0]).abs() < 1e-9);
        assert!((tilt([30.0, 77.0, 0.0]) - 30.0).abs() < 1e-9);
        assert!((tilt([0.0, 0.0, -20.0]) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn the_nearest_of_its_own_skips_one_placed_twice_and_other_maps() {
        let a = placement(0, [0.0, 0.0], 0.0, [0.0; 3]);
        let twice = placement(0, [0.0, 0.0], 0.0, [0.0; 3]);
        let b = placement(0, [3.0, 4.0], 0.0, [0.0; 3]);
        let far = placement(0, [500.0, 0.0], 0.0, [0.0; 3]);
        let d = nearest_of_its_own(&[&a, &twice, &b, &far], |_| 0);
        assert_eq!(d, vec![5.0, 5.0, 5.0], "the far one has none in reach");
        let d = nearest_of_its_own(&[&a, &b], |p| u32::from(p.position[0] > 1.0));
        assert!(d.is_empty(), "another map's is not near");
    }

    #[test]
    fn rules_read_back_as_written() {
        let dir = std::env::temp_dir().join(format!("cairn-fits-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rules = vec![(
            "World\\Tree.mdx".to_owned(),
            Rules {
                slope: [0.0, 38.0],
                apart: 4.5,
                scale: [0.8, 1.25],
                lean: true,
                placed: 12,
            },
        )];
        assert!(write(&dir, || rules.clone()).expect("written"));
        assert!(!write(&dir, || unreachable!()).expect("kept"));
        let back = read(&dir).expect("read");
        assert_eq!(back.of("WORLD/Tree.m2"), Some(rules[0].1));
        std::fs::write(
            dir.join(FILE),
            format!("{HEADER}\nA\t5..1\t1\t1..1\tupright\t1\n"),
        )
        .expect("write");
        assert!(read(&dir).is_err(), "a slope range backwards");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_table_of_rules_is_an_error_never_a_panic() {
        let dir = std::env::temp_dir().join(format!("cairn-fits-damaged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rule = |apart: f64| Rules {
            slope: [2.0, 41.0],
            apart,
            scale: [0.7, 1.3],
            lean: apart > 2.0,
            placed: 30,
        };
        write(&dir, || {
            vec![
                ("World\\A.mdx".to_owned(), rule(1.5)),
                ("World\\B.mdx".to_owned(), rule(12.0)),
            ]
        })
        .expect("written");
        let path = dir.join(FILE);
        let whole = std::fs::read(&path).expect("read");
        let (mut read_back, mut refused) = (0, 0);
        let mut copies: Vec<Vec<u8>> = (0..whole.len()).map(|cut| whole[..cut].to_vec()).collect();
        for at in 0..whole.len() {
            for bit in [0, 3, 6] {
                let mut b = whole.clone();
                b[at] ^= 1 << bit;
                copies.push(b);
            }
        }
        for b in copies {
            std::fs::write(&path, b).expect("write");
            match read(&dir) {
                Ok(book) => {
                    read_back += 1;
                    let r = book.of("World\\A.mdx").or(book.of("World\\B.mdx"));
                    assert!(r.is_none_or(|r| r.slope[0] <= r.slope[1] && r.apart > 0.0));
                }
                Err(_) => refused += 1,
            }
        }
        assert!(read_back > 0 && refused > 0, "{read_back} {refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
