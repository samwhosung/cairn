use super::*;

fn chunk(magic: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&magic);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn group(bytes: &[u8]) -> WmoGroup {
    match parse_wmo(bytes) {
        Ok(ParsedWmo::Group(g)) => g,
        other => panic!("expected a group, got {other:?}"),
    }
}

fn mliq_header(xverts: u32, yverts: u32, xtiles: u32, ytiles: u32) -> Vec<u8> {
    let mut mliq = Vec::new();
    for n in [xverts, yverts, xtiles, ytiles] {
        mliq.extend(n.to_le_bytes());
    }
    mliq.extend([10.0f32, 20.0, -5.0].map(f32::to_le_bytes).concat());
    mliq.extend(115u16.to_le_bytes());
    mliq
}

fn group_with_liquid(mliq: &[u8]) -> Vec<u8> {
    let mut mogp = vec![0u8; 68];
    mogp[0x34..0x38].copy_from_slice(&0xfu32.to_le_bytes());
    mogp.extend(chunk(*b"QILM", mliq));
    chunk(*b"PGOM", &mogp)
}

#[test]
fn root_parses_textures_materials_and_group_count() {
    let mut mohd = vec![0u8; 64];
    mohd[4..8].copy_from_slice(&7u32.to_le_bytes());
    for (i, v) in [-1.0f32, -2.0, 0.0, 3.0, 4.0, 5.5].iter().enumerate() {
        mohd[0x24 + 4 * i..0x28 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    let motx = b"a.blp\0\0b.blp\0";
    let mut momt = vec![0u8; 64];
    momt[8..12].copy_from_slice(&1u32.to_le_bytes());
    momt[12..16].copy_from_slice(&7u32.to_le_bytes());
    momt[0x10..0x14].copy_from_slice(&[1, 2, 3, 4]);
    momt[0x1c..0x20].copy_from_slice(&[5, 6, 7, 8]);
    momt[0x20..0x24].copy_from_slice(&10u32.to_le_bytes());
    let mut b = chunk(*b"DHOM", &mohd);
    b.extend(chunk(*b"XTOM", motx));
    b.extend(chunk(*b"TMOM", &momt));
    let Ok(ParsedWmo::Root(root)) = parse_wmo(&b) else {
        panic!("expected a root");
    };
    assert_eq!(root.n_groups, 7);
    assert_eq!(root.bounds, [[-1.0, -2.0, 0.0], [3.0, 4.0, 5.5]]);
    assert_eq!(root.textures, ["a.blp", "b.blp"]);
    let [m] = root.materials.as_slice() else {
        panic!("expected one material");
    };
    assert_eq!(m.blend_mode, 1);
    assert_eq!(m.sidn_rgb, [3, 2, 1]);
    assert_eq!(m.diff_color, [7, 6, 5]);
    assert_eq!(m.ground_type, 10);
    assert_eq!(m.texture_1_index(&root.texture_offset_index_map), Some(1));
}

#[test]
fn group_parses_geometry() {
    let mut mogp = vec![0u8; 68];
    mogp[8..12].copy_from_slice(&0x48u32.to_le_bytes());
    let vert = [1.0f32, 2.0, 3.0].map(f32::to_le_bytes).concat();
    mogp.extend(chunk(*b"TVOM", &vert));
    mogp.extend(chunk(*b"IVOM", &[0u8, 0, 1, 0, 2, 0]));
    mogp.extend(chunk(
        *b"VTOM",
        &[0.5f32, 0.25].map(f32::to_le_bytes).concat(),
    ));
    mogp.extend(chunk(
        *b"VTOM",
        &[1.0f32, 0.0].map(f32::to_le_bytes).concat(),
    ));
    let g = group(&chunk(*b"PGOM", &mogp));
    assert_eq!(g.flags, 0x48);
    assert_eq!(
        g.vertex_positions,
        [Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0
        }]
    );
    assert_eq!(g.vertex_indices, [0, 1, 2]);
    assert_eq!(g.texture_coords.len(), 2);
}

#[test]
fn group_parses_mliq_liquid_grid() {
    let mut mliq = mliq_header(2, 2, 1, 1);
    for opacity in [86u8, 86, 90, 90] {
        mliq.extend([opacity, 0, 0, 0]);
        mliq.extend((-5.0f32).to_le_bytes());
    }
    mliq.push(0x44);
    let g = group(&group_with_liquid(&mliq));
    assert_eq!(g.group_liquid, 0xf);
    let lq = g.liquid.expect("has liquid");
    assert_eq!((lq.xverts, lq.yverts, lq.xtiles, lq.ytiles), (2, 2, 1, 1));
    assert_eq!(
        lq.base.map(f32::to_bits),
        [10.0f32, 20.0, -5.0].map(f32::to_bits)
    );
    assert_eq!(lq.material_id, 115);
    assert!(
        lq.heights
            .iter()
            .all(|&h| h.to_bits() == (-5.0f32).to_bits())
    );
    assert_eq!(lq.opacity, [86, 86, 90, 90]);
    assert_eq!(lq.tile_flags, [0x44]);
}

#[test]
fn liquid_grid_past_the_payload_is_no_liquid() {
    let g = group(&group_with_liquid(&mliq_header(9, 9, 8, 8)));
    assert!(g.liquid.is_none());
}

#[test]
fn liquid_header_cut_after_the_grid_size_is_no_liquid() {
    let mliq = mliq_header(2, 2, 1, 1);
    let g = group(&group_with_liquid(&mliq[..20]));
    assert!(g.liquid.is_none());
}

#[test]
fn truncated_mohd_is_an_error() {
    let b = chunk(*b"DHOM", &[0u8; 3]);
    assert_eq!(parse_wmo(&b), Err(Error::Truncated("MOHD")));
}

#[test]
fn hostile_shapes_do_not_panic() {
    assert_eq!(parse_wmo(&[]), Err(Error::NotWmo));
    assert_eq!(parse_wmo(&[0u8; 7]), Err(Error::NotWmo));
    let mut b = chunk(*b"DHOM", &[0u8; 64]);
    b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(parse_wmo(&b), Ok(ParsedWmo::Root(_))));
    let g = group(&chunk(*b"PGOM", &[0u8; 10]));
    assert!(g.vertex_positions.is_empty());
    assert_eq!(g.group_liquid, 0xf);
}
