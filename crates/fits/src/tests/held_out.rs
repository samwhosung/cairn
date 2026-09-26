use std::collections::HashMap;
use std::path::PathBuf;

use super::*;

const RANK_ONE_IN: usize = 4;
const MIN_FIRST_PAGE_SHARE: f64 = 0.85;
const LISTS: &str = "the lists: palette, ground, 8 yd else 20";
const EASTERN_KINGDOMS: u32 = 0;
const KALIMDOR: u32 = 1;
const HOLD_OUT_ONE_IN: u64 = 5;

fn held_out(map: u32, (x, y): (u32, u32)) -> bool {
    let hash = tally::fnv([map, x, y].iter().flat_map(|v| v.to_le_bytes()));
    [EASTERN_KINGDOMS, KALIMDOR].contains(&map) && hash % HOLD_OUT_ONE_IN == 0
}

fn plus(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut sum = a.to_vec();
    score::add(&mut sum, b);
    sum
}

struct Orders(Vec<Tally>);

impl Orders {
    fn new<S: AsRef<str>>(names: &[S]) -> Self {
        Self(names.iter().map(|n| Tally::new(n.as_ref())).collect())
    }

    fn add(&mut self, t: &Tables, placed: usize, lists: &[Vec<f64>]) {
        for (tally, score) in self.0.iter_mut().zip(lists) {
            tally.add(t, score, placed);
        }
    }

    fn show(&self, title: &str) {
        eprintln!("\n{title}\n{HEADER}");
        for t in &self.0 {
            eprintln!("{}", t.row());
        }
    }

    fn get(&self, name: &str) -> &Tally {
        self.0
            .iter()
            .find(|t| t.name.starts_with(name))
            .expect("a list of that name")
    }

    fn page(&self, name: &str) -> f64 {
        self.get(name).hit(20, true)
    }

    fn ten(&self, name: &str) -> f64 {
        self.get(name).hit(10, false)
    }
}

struct Split {
    kept: Tables,
    held_out: Vec<Stand>,
}

fn split(survey: &survey::Survey) -> Split {
    let (models, grounds, zones) = lists(survey);
    let (mut kept, mut out) = (Vec::new(), Vec::new());
    for s in stands(survey) {
        let tile = wdt::world_to_tile(s.at[0], s.at[1]);
        if held_out(zones[s.zone].map, tile) {
            out.push(s);
        } else {
            kept.push(s);
        }
    }
    Split {
        kept: Tables::count(models, grounds, zones, &kept),
        held_out: out,
    }
}

fn sampled_spots<'a>(t: &'a Tables, test: &'a [Stand]) -> impl Iterator<Item = (usize, Spot)> + 'a {
    let cell = |s: &Stand| {
        let c = |v: f32| (v / AROUND).floor() as i32;
        (t.zones[s.zone].map, c(s.at[0]), c(s.at[1]))
    };
    let mut grid: HashMap<(u32, i32, i32), Vec<usize>> = HashMap::new();
    for (i, s) in test.iter().enumerate() {
        grid.entry(cell(s)).or_default().push(i);
    }
    test.iter()
        .enumerate()
        .step_by(RANK_ONE_IN)
        .map(move |(i, s)| {
            let (map, cx, cy) = cell(s);
            let mut near = Vec::new();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &j in grid.get(&(map, cx + dx, cy + dy)).into_iter().flatten() {
                        let o = &test[j];
                        let d = (s.at[0] - o.at[0]).hypot(s.at[1] - o.at[1]);
                        if j != i && d <= AROUND {
                            near.push((o.model, d));
                        }
                    }
                }
            }
            near.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            let spot = Spot {
                zone: Some(s.zone),
                ground: s.ground,
                near,
            };
            (s.model, spot)
        })
}

#[test]
fn the_model_placed_on_a_held_out_tile_heads_its_kinds_list() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let chain = mpq::Chain::open(data).expect("the install");
    let survey = survey::read(&chain).expect("the survey");
    let Split {
        kept: t,
        held_out: test,
    } = split(&survey);
    let own = Own::default();
    let ev = Evidence::new(&t, &own, true);
    let control = shuffled(&t);
    let everywhere = ev.everywhere();
    let mut near = Orders::new(&[
        "shuffled (the control)",
        "most placed first",
        "the zone's palette",
        "the ground under it",
        "what stands within 8 yd",
        "palette, ground and within 8 yd",
        LISTS,
    ]);
    let kinds: Vec<String> = MODEL_KINDS[..5]
        .iter()
        .map(|k| format!("the lists, {k}s"))
        .collect();
    let mut by_kind = Orders::new(&kinds);
    let mut around = Orders::new(&[
        "shuffled (the control)",
        "most placed first",
        "the zone's palette",
        "what stands within 20 yd",
        LISTS,
    ]);
    let mut buildings = Orders::new(&[
        "shuffled (the control)",
        "most placed first",
        "the zone's palette",
        LISTS,
    ]);
    for (placed, spot) in sampled_spots(&t, &test) {
        let palette = ev.palette(spot.zone);
        let lists = ev.scores(&spot);
        let first = [control.clone(), everywhere.clone(), palette.clone()];
        if t.models[placed].kind == "building" {
            buildings.add(&t, placed, &[&first[..], &[lists]].concat());
            continue;
        }
        let beside = spot.beside(Reach::Near);
        if beside.is_empty() {
            if !spot.near.is_empty() {
                let neighbours = spot.beside(Reach::Around);
                let within = plus(&everywhere, &ev.beside(Reach::Around, &neighbours));
                around.add(&t, placed, &[&first[..], &[within, lists]].concat());
            }
            continue;
        }
        let ground = spot
            .ground
            .map_or_else(|| vec![0.0; t.models.len()], |g| ev.ground(g));
        let beside = ev.beside(Reach::Near, &beside);
        let all = plus(&plus(&palette, &ground), &beside);
        if let Some(k) = t.kind_of(placed).filter(|&k| k < kinds.len()) {
            by_kind.0[k].add(&t, &lists, placed);
        }
        let more = [
            plus(&everywhere, &ground),
            plus(&everywhere, &beside),
            all,
            lists,
        ];
        near.add(&t, placed, &[&first[..], &more].concat());
    }
    near.show("doodads on held-out tiles with a neighbour within 8 yd");
    by_kind.show("the same, by kind");
    around.show("doodads with none within 8 yd but one within 20");
    buildings.show("buildings on held-out tiles");
    for o in [&near, &around, &buildings] {
        assert!(o.page(LISTS) > o.page("most placed") + 0.1);
        assert!(o.page("most placed") > o.page("shuffled"));
    }
    for o in [&near, &around] {
        assert!(o.ten(LISTS) > o.ten("most placed"));
        assert!(o.ten("most placed") > o.ten("shuffled"));
    }
    assert!(near.page(LISTS) >= MIN_FIRST_PAGE_SHARE);
    assert!(
        near.page("shuffled") < MIN_FIRST_PAGE_SHARE,
        "the control fails the same bar"
    );
    assert!(near.get(LISTS).len() > 5_000, "enough ranked to tell");
}
