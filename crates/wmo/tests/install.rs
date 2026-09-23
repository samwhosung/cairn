use std::path::PathBuf;

use mpq::Chain;
use wmo::{ParsedWmo, WmoGroup, WmoRoot, parse_wmo};

const STORMWIND: &str = "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind";

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn root(chain: &Chain, name: &str) -> WmoRoot {
    let bytes = chain.read(name).unwrap_or_else(|e| panic!("{e}"));
    match parse_wmo(&bytes) {
        Ok(ParsedWmo::Root(r)) => r,
        other => panic!("{name}: expected a root, got {:?}", other.map(|_| ())),
    }
}

fn group(chain: &Chain, root_name: &str, index: u32) -> WmoGroup {
    let name = format!("{}_{index:03}.wmo", root_name.trim_end_matches(".wmo"));
    let bytes = chain.read(&name).unwrap_or_else(|e| panic!("{e}"));
    match parse_wmo(&bytes) {
        Ok(ParsedWmo::Group(g)) => g,
        other => panic!("{name}: expected a group, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn stormwind_root_names_its_groups_and_textures() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let r = root(&chain, &format!("{STORMWIND}.wmo"));
    assert_eq!(r.n_groups, 306);
    assert_eq!(r.textures.len(), 155);
    assert_eq!(r.materials.len(), 194);
    for m in &r.materials {
        let index = m.texture_1_index(&r.texture_offset_index_map);
        assert!(
            index.is_some_and(|i| (i as usize) < r.textures.len()),
            "{m:?}"
        );
    }
}

#[test]
fn stormwind_groups_hold_consistent_geometry() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let name = format!("{STORMWIND}.wmo");
    let r = root(&chain, &name);
    let mut with_liquid = 0;
    for i in 0..r.n_groups {
        let g = group(&chain, &name, i);
        let verts = g.vertex_positions.len();
        assert!(
            g.vertex_indices.iter().all(|&v| usize::from(v) < verts),
            "group {i}"
        );
        assert_eq!(
            g.material_info.len(),
            g.vertex_indices.len() / 3,
            "group {i}"
        );
        for b in &g.render_batches {
            let end = b.start_index as usize + usize::from(b.count);
            assert!(end <= g.vertex_indices.len(), "group {i}: {b:?}");
            assert!(
                usize::from(b.material_id) < r.materials.len(),
                "group {i}: {b:?}"
            );
        }
        with_liquid += usize::from(g.liquid.is_some());
    }
    assert_eq!(with_liquid, 22);
}

#[test]
fn stormwind_canal_liquid_grid() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let g = group(&chain, &format!("{STORMWIND}.wmo"), 103);
    assert_eq!(g.group_liquid, 0xf);
    let lq = g.liquid.expect("the canal group has liquid");
    assert_eq!(
        (lq.xverts, lq.yverts, lq.xtiles, lq.ytiles),
        (28, 14, 27, 13)
    );
    assert_eq!(lq.heights.len(), 28 * 14);
    assert_eq!(lq.opacity.len(), 28 * 14);
    assert_eq!(lq.tile_flags.len(), 27 * 13);
    let mut histogram = [0usize; 256];
    for &o in &lq.opacity {
        histogram[usize::from(o)] += 1;
    }
    assert_eq!(histogram[86], 315);
}
