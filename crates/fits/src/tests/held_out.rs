use std::collections::HashMap;
use std::path::PathBuf;

use super::*;

/// One placement in this many on the held-out tiles is ranked.
const EVERY: usize = 4;
/// The share of held-out doodads with a neighbour within 8 yd whose model the lists must put on
/// the first page of its kind.
const BAR: f64 = 0.85;
const LISTS: &str = "the lists: palette, ground, 8 yd else 20";

/// A fifth of the two continents' tiles, picked by a hash of the map and the tile.
fn held_out(map: u32, (x, y): (u32, u32)) -> bool {
    map <= 1 && tally::fnv([map, x, y].iter().flat_map(|v| v.to_le_bytes())) % 5 == 0
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

/// The tables counted on the tiles kept, and the placements on the tiles held out.
fn split(survey: &survey::Survey) -> (Tables, Vec<Stand>) {
    let (models, grounds, zones) = lists(survey);
    let (mut train, mut test) = (Vec::new(), Vec::new());
    for s in stands(survey) {
        let tile = wdt::world_to_tile(s.at[0], s.at[1]);
        if held_out(zones[s.zone].map, tile) {
            test.push(s);
        } else {
            train.push(s);
        }
    }
    (Tables::count(models, grounds, zones, &train), test)
}

/// Every held-out placement's spot, with what else on the held-out tiles stands around it.
fn spots<'a>(t: &'a Tables, test: &'a [Stand]) -> impl Iterator<Item = (usize, Spot)> + 'a {
    let cell = |s: &Stand| {
        let c = |v: f32| (v / AROUND).floor() as i32;
        (t.zones[s.zone].map, c(s.at[0]), c(s.at[1]))
    };
    let mut grid: HashMap<(u32, i32, i32), Vec<usize>> = HashMap::new();
    for (i, s) in test.iter().enumerate() {
        grid.entry(cell(s)).or_default().push(i);
    }
    test.iter().enumerate().step_by(EVERY).map(move |(i, s)| {
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
    let (t, test) = split(&survey);
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
    let kinds: Vec<String> = KINDS[..5]
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
    for (placed, spot) in spots(&t, &test) {
        let palette = ev.palette(spot.zone);
        let lists = ev.scores(&spot);
        let first = [control.clone(), everywhere.clone(), palette.clone()];
        if t.models[placed].kind == "building" {
            buildings.add(&t, placed, &[&first[..], &[lists]].concat());
            continue;
        }
        let beside = spot.beside(NEAR);
        if beside.is_empty() {
            if !spot.near.is_empty() {
                let within = plus(&everywhere, &ev.beside(AROUND, &spot.beside(AROUND)));
                around.add(&t, placed, &[&first[..], &[within, lists]].concat());
            }
            continue;
        }
        let ground = spot
            .ground
            .map_or_else(|| vec![0.0; t.models.len()], |g| ev.ground(g));
        let beside = ev.beside(NEAR, &beside);
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
    assert!(near.page(LISTS) >= BAR, "on its kind's first page");
    assert!(
        near.page("shuffled") < BAR,
        "the control fails the same bar"
    );
    assert!(near.get(LISTS).len() > 5_000, "enough ranked to tell");
}
