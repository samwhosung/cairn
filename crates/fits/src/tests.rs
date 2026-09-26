use std::path::{Path, PathBuf};

use super::*;

mod held_out;

fn model(kind: &str, path: &str) -> Model {
    Model {
        kind: kind.to_owned(),
        path: path.to_owned(),
    }
}

fn zone(map: u32, area: u32, name: &str) -> Zone {
    Zone {
        map,
        area,
        key: name.to_ascii_lowercase(),
        name: name.to_owned(),
    }
}

fn stand(model: usize, zone: usize, at: [f32; 2]) -> Stand {
    Stand {
        model,
        zone,
        at,
        ground: Some((GRASS, band(5.0))),
    }
}

const WELL: usize = 0;
const JAR: usize = 1;
const BUCKET: usize = 2;
const CART: usize = 3;
const CRATE: usize = 4;
const OAK: usize = 5;
const PINE: usize = 6;
const FARM: usize = 7;
const VALE: usize = 0;
const MOOR: usize = 1;
const COAST: usize = 2;
const GRASS: usize = 0;

fn village() -> Tables {
    let models = vec![
        model("prop", "World\\Well.mdx"),
        model("prop", "World\\Jar.mdx"),
        model("prop", "World\\Bucket.mdx"),
        model("prop", "World\\Cart.mdx"),
        model("prop", "World\\Crate.mdx"),
        model("tree", "World\\Oak.mdx"),
        model("tree", "World\\Pine.mdx"),
        model("building", "World\\Farm.wmo"),
    ];
    let zones = vec![
        zone(0, 12, "Vale"),
        zone(0, 40, "Moor"),
        zone(1, 14, "Coast"),
    ];
    let mut stands = Vec::new();
    for i in 0..6 {
        let x = 100.0 * i as f32;
        stands.push(stand(WELL, VALE, [x, 0.0]));
        stands.push(stand(JAR, VALE, [x + 1.5, 0.0]));
        stands.push(stand(BUCKET, VALE, [x, 10.0]));
        stands.push(stand(FARM, MOOR, [x, 1000.0]));
        stands.push(stand(CART, MOOR, [x + 12.0, 1000.0]));
        stands.push(stand(CRATE, MOOR, [x + 30.0, 1000.0]));
        stands.push(stand(OAK, VALE, [x + 50.0, 50.0]));
        stands.push(stand(OAK, VALE, [x + 56.0, 50.0]));
        stands.push(stand(PINE, COAST, [x, 0.0]));
    }
    Tables::count(models, vec!["Tileset\\Grass.blp".into()], zones, &stands)
}

#[test]
fn every_two_placements_around_each_other_on_one_map_pair_once() {
    let t = village();
    let well_jar = t.pair(WELL, JAR).expect("wells have jars");
    assert_eq!((well_jar.near, well_jar.around), (6, 6));
    assert_eq!(t.usual(WELL, JAR), Some(1.5), "the jar from the well");
    assert_eq!(t.usual(JAR, BUCKET), Some(10.1), "the bucket from the jar");
    let bucket = t.pair(WELL, BUCKET).expect("buckets by wells");
    assert_eq!((bucket.near, bucket.around), (0, 6));
    let oaks = t.pair(OAK, OAK).expect("oaks in pairs");
    assert_eq!((oaks.near, oaks.around, oaks.a_to_b), (6, 6, 6.0));
    assert_eq!(oaks.seen(Reach::Near), 12, "each oak sees the other");
    assert_eq!(t.seen_beside(OAK, Reach::Near), 12);
    assert!(
        t.pair(WELL, PINE).is_none(),
        "the pine stands on another map"
    );
    assert!(t.pair(WELL, OAK).is_none(), "the oaks stand 70 yd off");
    assert_eq!(t.placed(OAK), 12);
    assert_eq!((t.in_zone(VALE, WELL), t.in_zone(MOOR, WELL)), (6, 0));
    assert_eq!(
        t.model("world/WELL.m2"),
        Some(WELL),
        "any case, slash and extension"
    );
}

#[test]
fn what_stands_beside_a_spot_ranks_what_goes_with_it() {
    let t = village();
    let own = Own::default();
    let lists = Evidence::new(&t, &own, true);
    let by_the_well = Spot {
        zone: Some(VALE),
        ground: Some((GRASS, 0)),
        near: vec![(WELL, 1.4)],
    };
    let first = lists.list(&by_the_well, t.kind_of(JAR), 2);
    assert_eq!(first[0].model, JAR, "a jar by a well");
    let beside = first[0].beside.expect("why");
    assert_eq!((beside.model, beside.usual), (WELL, Some(1.5)));
    let by_the_farm = Spot {
        zone: Some(MOOR),
        ground: None,
        near: vec![(FARM, 12.0)],
    };
    assert_eq!(lists.list(&by_the_farm, t.kind_of(CART), 1)[0].model, CART);
    let order = lists.list(&by_the_farm, None, 8);
    assert_eq!(order.len(), 8, "every kind");
}

const PAIRS_TO_OUTWEIGH_PRIOR: usize = 40;

#[test]
fn a_zone_of_its_own_counts_what_it_placed() {
    let t = village();
    let mut own = Own::default();
    for i in 0..PAIRS_TO_OUTWEIGH_PRIOR {
        let at = [5000.0 + 30.0 * i as f32, 0.0];
        own.place(&format!("crate {i}"), CRATE, at, None);
        own.place(&format!("pine {i}"), PINE, [at[0] + 2.0, at[1]], None);
    }
    own.place("lone crate", CRATE, [9000.0, 0.0], None);
    assert_eq!(own.len(), 2 * PAIRS_TO_OUTWEIGH_PRIOR + 1);
    let spot = Spot {
        zone: Some(VALE),
        ground: None,
        near: own.around([9003.0, 0.0]),
    };
    let borrowing = Evidence::new(&t, &own, true);
    assert_eq!(
        borrowing.list(&spot, t.kind_of(PINE), 1)[0].model,
        PINE,
        "the pines the zone placed, before the oaks the borrowed zone has"
    );
    let alone = Evidence::new(&t, &own, false);
    let list = alone.list(&spot, None, 3);
    assert_eq!(
        list[0].model, PINE,
        "a pine beside a crate, as the zone does"
    );
    assert_eq!(list[0].beside.and_then(|b| b.usual), Some(2.0));
    let others: Vec<usize> = alone.list(&spot, None, 8).iter().map(|f| f.model).collect();
    assert_eq!(
        &others[2..],
        [WELL, JAR, BUCKET, CART, OAK, FARM],
        "what it never placed, in no order"
    );
    for i in 0..PAIRS_TO_OUTWEIGH_PRIOR {
        assert!(own.remove(&format!("crate {i}")));
        assert!(own.remove(&format!("pine {i}")));
    }
    assert!(own.remove("lone crate") && !own.remove("lone crate"));
    assert_eq!(
        own,
        Own::default(),
        "taking everything away leaves nothing counted"
    );
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fits-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_tables_read_back_as_written() {
    let t = village();
    let dir = scratch("round");
    assert_eq!(write(&dir, || t.clone()), Ok(FILES.len()));
    assert_eq!(read(&dir).as_ref(), Ok(&t));
    assert_eq!(write(&dir, || unreachable!("nothing is missing")), Ok(0));
    std::fs::remove_file(dir.join("near.tsv")).expect("remove");
    assert_eq!(write(&dir, || t.clone()), Ok(1));
    assert_eq!(read(&dir), Ok(t));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_table_is_an_error_never_a_panic() {
    let dir = scratch("damaged");
    write(&dir, village).expect("written");
    for (name, _) in FILES {
        let path = dir.join(name);
        let whole = std::fs::read(&path).expect("read");
        let mut tried = 0;
        for cut in (0..whole.len()).step_by(7) {
            damage(&path, &whole[..cut], &dir);
            tried += 1;
        }
        for at in (0..whole.len()).step_by(3) {
            for bit in [0, 3, 6] {
                let mut flipped = whole.clone();
                flipped[at] ^= 1 << bit;
                damage(&path, &flipped, &dir);
                tried += 1;
            }
        }
        std::fs::write(&path, &whole).expect("restore");
        assert!(tried > 3, "{name}");
    }
    assert!(read(&dir).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

fn damage(path: &Path, bytes: &[u8], dir: &Path) {
    std::fs::write(path, bytes).expect("write");
    if let Ok(t) = read(dir) {
        let own = Own::default();
        let lists = Evidence::new(&t, &own, true);
        let spot = Spot {
            zone: Some(VALE),
            ground: Some((GRASS, 0)),
            near: vec![(WELL, 1.0), (JAR, 12.0)],
        };
        let _ = lists.list(&spot, None, 20);
    }
}

#[test]
fn slopes_fall_in_bands() {
    assert_eq!(
        [0.0, 9.9, 10.0, 44.9, 45.0, 80.0].map(band),
        [0, 0, 1, 3, 4, 4]
    );
    assert_eq!(band_name(3), "30-45");
}
