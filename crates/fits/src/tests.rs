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
        ground: Some((0, band(5.0))),
    }
}

/// Wells with a jar a yard and a half off and a bucket ten off, a cart by a farm and a crate
/// farther, oaks in pairs: in two zones of one map, and pines on another.
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
        stands.push(stand(0, 0, [x, 0.0]));
        stands.push(stand(1, 0, [x + 1.5, 0.0]));
        stands.push(stand(2, 0, [x, 10.0]));
        stands.push(stand(7, 1, [x, 1000.0]));
        stands.push(stand(3, 1, [x + 12.0, 1000.0]));
        stands.push(stand(4, 1, [x + 30.0, 1000.0]));
        stands.push(stand(5, 0, [x + 50.0, 50.0]));
        stands.push(stand(5, 0, [x + 56.0, 50.0]));
        stands.push(stand(6, 2, [x, 0.0]));
    }
    Tables::count(models, vec!["Tileset\\Grass.blp".into()], zones, &stands)
}

#[test]
fn every_two_placements_around_each_other_on_one_map_pair_once() {
    let t = village();
    let well_jar = t.pair(0, 1).expect("wells have jars");
    assert_eq!((well_jar.near, well_jar.around), (6, 6));
    assert_eq!(t.usual(0, 1), Some(1.5), "the jar from the well");
    assert_eq!(t.usual(1, 2), Some(10.1), "the bucket from the jar");
    assert_eq!(t.pair(0, 2).map(|p| (p.near, p.around)), Some((0, 6)));
    let oaks = t.pair(5, 5).expect("oaks in pairs");
    assert_eq!((oaks.near, oaks.around, oaks.a_to_b), (6, 6, 6.0));
    assert!(t.pair(0, 6).is_none(), "the pine stands on another map");
    assert!(t.pair(0, 5).is_none(), "the oaks stand 70 yd off");
    assert_eq!(t.placed(5), 12);
    assert_eq!((t.in_zone(0, 0), t.in_zone(1, 0)), (6, 0));
    assert_eq!(
        t.model("world/WELL.m2"),
        Some(0),
        "any case, slash and extension"
    );
}

#[test]
fn what_stands_beside_a_spot_ranks_what_goes_with_it() {
    let t = village();
    let own = Own::default();
    let lists = Evidence::new(&t, &own, true);
    let by_the_well = Spot {
        zone: Some(0),
        ground: Some((0, 0)),
        near: vec![(0, 1.4)],
    };
    let first = lists.list(&by_the_well, t.kind_of(1), 2);
    assert_eq!(first[0].model, 1, "a jar by a well");
    let beside = first[0].beside.expect("why");
    assert_eq!((beside.model, beside.usual), (0, Some(1.5)));
    let by_the_farm = Spot {
        zone: Some(1),
        ground: None,
        near: vec![(7, 12.0)],
    };
    assert_eq!(lists.list(&by_the_farm, t.kind_of(3), 1)[0].model, 3);
    let order = lists.list(&by_the_farm, None, 8);
    assert_eq!(order.len(), 8, "every kind");
}

/// Enough pairs for the zone's own to outweigh how common each model is.
const PAIRED: usize = 40;

#[test]
fn a_zone_of_its_own_counts_what_it_placed() {
    let t = village();
    let mut own = Own::default();
    for i in 0..PAIRED {
        let at = [5000.0 + 30.0 * i as f32, 0.0];
        own.place(&format!("crate {i}"), 4, at, None);
        own.place(&format!("pine {i}"), 6, [at[0] + 2.0, at[1]], None);
    }
    own.place("lone crate", 4, [9000.0, 0.0], None);
    assert_eq!(own.len(), 2 * PAIRED + 1);
    let spot = Spot {
        zone: Some(0),
        ground: None,
        near: own.around([9003.0, 0.0]),
    };
    let borrowing = Evidence::new(&t, &own, true);
    assert_eq!(
        borrowing.list(&spot, t.kind_of(6), 1)[0].model,
        6,
        "the pines the zone placed, before the oaks the borrowed zone has"
    );
    let alone = Evidence::new(&t, &own, false);
    let list = alone.list(&spot, None, 3);
    assert_eq!(list[0].model, 6, "a pine beside a crate, as the zone does");
    assert_eq!(list[0].beside.and_then(|b| b.usual), Some(2.0));
    let others: Vec<usize> = alone.list(&spot, None, 8).iter().map(|f| f.model).collect();
    assert_eq!(
        &others[2..],
        [0, 1, 2, 3, 5, 7],
        "what it never placed, in no order"
    );
    for i in 0..PAIRED {
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
            zone: Some(0),
            ground: Some((0, 0)),
            near: vec![(0, 1.0), (1, 12.0)],
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
