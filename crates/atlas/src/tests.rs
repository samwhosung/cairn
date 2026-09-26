use terrain::{Doodad, LiquidKind, LiquidMesh, WmoInstance};

use super::*;

const TILE: (u32, u32) = (32, 32);
const ZONE: u32 = 1;
const ELSEWHERE: u32 = 2;

fn areas() -> Areas {
    let area = |name: &str| Area {
        map: 0,
        parent: None,
        name: name.into(),
    };
    Areas::from_rows([(ZONE, area("Here")), (ELSEWHERE, area("There"))])
}

fn south_east_of_corner(south: f32, east: f32) -> [f32; 2] {
    let [x, y] = origin(TILE.0, TILE.1);
    [x - south, y - east]
}

fn chunk(row: u32, col: u32, area: u32) -> ChunkMesh {
    let [x, y] = south_east_of_corner(row as f32 * CHUNK_SIZE, col as f32 * CHUNK_SIZE);
    ChunkMesh {
        positions: vec![[x, y, 0.0]; 145],
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
        holes: 0,
        base_texture: Some("red".into()),
        layer_textures: vec!["red".into()],
        layer_effect_ids: vec![0],
        alpha_map: None,
        shadow: None,
        pred_tex: [0; 64],
        no_effect_doodad: [false; 64],
        index_x: col,
        index_y: row,
        area_id: area,
        impassable: false,
        liquids: Vec::new(),
    }
}

fn pond(depth: f32) -> LiquidMesh {
    LiquidMesh {
        grid: [9, 9],
        wet: vec![true; 64],
        shared: vec![false; 64],
        positions: vec![[0.0, 0.0, depth]; 81],
        uvs: Vec::new(),
        depths: Vec::new(),
        indices: Vec::new(),
        sound_nibble: 0,
        material_id: None,
        kind: LiquidKind::Still,
    }
}

fn tile(chunks: Vec<ChunkMesh>, doodads: Vec<Doodad>, wmos: Vec<WmoInstance>) -> TileMesh {
    TileMesh {
        chunks,
        doodads,
        wmos,
    }
}

fn draw(t: TileMesh, ypp: f32) -> (RgbImage, Doodads) {
    let loaded = vec![(TILE, t)];
    let colors = HashMap::from([("red".to_owned(), [1.0, 0.0, 0.0])]);
    let f = frame(&loaded, &areas(), Some(ZONE)).expect("the zone is on the tile");
    render(&loaded, &colors, &areas(), Some(ZONE), &f, ypp).expect("draws")
}

#[test]
fn a_chunk_is_drawn_in_its_texture_under_the_light_and_dimmed_outside_the_zone() {
    let mut wet = chunk(0, 2, ZONE);
    wet.liquids.push(pond(6.0));
    let chunks = vec![chunk(0, 0, ZONE), chunk(0, 1, ELSEWHERE), wet];
    let (img, counts) = draw(tile(chunks, Vec::new(), Vec::new()), CHUNK_SIZE / 2.0);
    assert_eq!(img.dimensions(), (32, 32));
    assert_eq!(counts, Doodads::default());
    assert_eq!(
        img.get_pixel(0, 0).0,
        [232, 0, 0],
        "lit from the north-west"
    );
    assert_eq!(img.get_pixel(2, 0).0, [34, 34, 38], "grey outside the zone");
    assert_eq!(
        img.get_pixel(4, 0).0,
        [129, 28, 44],
        "half-tinted by six yards of water"
    );
    assert_eq!(img.get_pixel(0, 2).0, [0, 0, 0], "no chunk");
}

#[test]
fn a_map_drawn_whole_dims_nothing_and_frames_every_tile() {
    let chunks = || vec![chunk(0, 0, ZONE), chunk(0, 1, ELSEWHERE)];
    let other = (TILE.0 + 1, TILE.1);
    let loaded = vec![
        (TILE, tile(chunks(), Vec::new(), Vec::new())),
        (
            other,
            tile(vec![chunk(0, 16, ELSEWHERE)], Vec::new(), Vec::new()),
        ),
    ];
    let colors = HashMap::from([("red".to_owned(), [1.0, 0.0, 0.0])]);
    let ypp = CHUNK_SIZE / 2.0;
    let whole = frame(&loaded, &areas(), None).expect("the map has terrain");
    assert_eq!((whole.x0, whole.x1, whole.y0, whole.y1), (32, 33, 32, 32));
    let (img, _) = render(&loaded, &colors, &areas(), None, &whole, ypp).expect("draws");
    assert_eq!(img.get_pixel(2, 0).0, [232, 0, 0], "in its colour");
    assert_eq!(img.get_pixel(32, 0).0, [232, 0, 0], "the next tile too");
    let zone = frame(&loaded, &areas(), Some(ZONE)).expect("the zone");
    let (img, _) = render(&loaded, &colors, &areas(), Some(ZONE), &zone, ypp).expect("draws");
    assert_eq!(img.get_pixel(2, 0).0, [34, 34, 38], "a zone dims the rest");
    assert!(frame(&[], &areas(), None).is_err());
}

#[test]
fn doodads_are_dots_by_kind_and_buildings_are_squares() {
    let doodad = |model: &str, [x, y]: [f32; 2], unique_id: u32| Doodad {
        model: model.into(),
        position: [x, y, 0.0],
        rotation: [0.0; 3],
        scale: 1.0,
        unique_id,
    };
    let chunks = (0..16)
        .flat_map(|r| (0..16).map(move |c| chunk(r, c, ZONE)))
        .collect();
    let doodads = vec![
        doodad("A\\OakTree.mdx", south_east_of_corner(100.5, 100.5), 1),
        doodad("A\\Barrel.mdx", south_east_of_corner(200.5, 100.5), 2),
    ];
    let [x, y] = south_east_of_corner(300.5, 300.5);
    let wmos = vec![WmoInstance {
        model: "A\\Inn.wmo".into(),
        position: [x, y, 0.0],
        rotation: [0.0; 3],
        unique_id: 0,
        doodad_set: 0,
        name_set: 0,
    }];
    let (img, counts) = draw(tile(chunks, doodads, wmos), 1.0);
    let one_each = Doodads {
        trees: 1,
        props: 1,
        ..Doodads::default()
    };
    assert_eq!(counts, one_each);
    assert_eq!(img.get_pixel(100, 100).0, [20, 70, 25]);
    assert_eq!(img.get_pixel(100, 200).0, [230, 170, 40]);
    assert_eq!(img.get_pixel(303, 303).0, [200, 30, 30]);
    assert_ne!(
        img.get_pixel(300, 300).0,
        [200, 30, 30],
        "a square, not a block"
    );
}

#[test]
fn the_census_counts_each_doodad_once_and_only_in_the_zone() {
    let chunks = (0..16)
        .flat_map(|r| (0..16).map(move |c| chunk(r, c, if c < 8 { ZONE } else { ELSEWHERE })))
        .collect();
    let tree = |[x, y]: [f32; 2], unique_id: u32| Doodad {
        model: "A\\OakTree.mdx".into(),
        position: [x, y, 0.0],
        rotation: [0.0; 3],
        scale: 1.0,
        unique_id,
    };
    let (here, there) = (
        south_east_of_corner(100.5, 10.5),
        south_east_of_corner(100.5, 400.5),
    );
    let listed = vec![tree(here, 1), tree(here, 1), tree(there, 2)];
    let (_, counts) = draw(tile(chunks, listed, Vec::new()), 4.0);
    let one_tree = Doodads {
        trees: 1,
        ..Doodads::default()
    };
    assert_eq!(counts, one_tree, "listed twice, and one elsewhere");
}

#[test]
fn a_map_too_large_or_too_small_is_refused() {
    let loaded = vec![(TILE, tile(vec![chunk(0, 0, ZONE)], Vec::new(), Vec::new()))];
    let f = frame(&loaded, &areas(), Some(ZONE)).expect("the zone is on the tile");
    let colors = HashMap::new();
    for ypp in [0.05, 2000.0, 0.0, -1.0, f32::NAN] {
        assert!(
            render(&loaded, &colors, &areas(), Some(ZONE), &f, ypp).is_err(),
            "{ypp}"
        );
    }
    assert!(frame(&loaded, &areas(), Some(ELSEWHERE)).is_err());
}

#[test]
fn a_mark_rings_its_point_when_it_is_on_the_map() {
    let f = Frame {
        x0: TILE.0,
        x1: TILE.0,
        y0: TILE.1,
        y1: TILE.1,
    };
    let mut img = RgbImage::new(64, 64);
    assert!(mark(&mut img, &f, 1.0, south_east_of_corner(30.5, 20.5)));
    assert_eq!(
        img.get_pixel(20, 30).0,
        [0, 0, 0],
        "the point itself stays clear"
    );
    assert_eq!(img.get_pixel(30, 30).0, RING, "ten pixels east");
    assert_eq!(img.get_pixel(20, 21).0, RING, "nine pixels north");
    assert_eq!(img.get_pixel(20, 42).0, [0, 0, 0], "twelve pixels south");
    let mut blank = RgbImage::new(64, 64);
    for (south, east) in [(-100.0, 0.0), (1.0e30, 0.0), (-0.5, 10.0), (10.0, -0.5)] {
        let point = south_east_of_corner(south, east);
        assert!(
            !mark(&mut blank, &f, 1.0, point),
            "{south} south, {east} east"
        );
    }
    assert!(blank.pixels().all(|p| p.0 == [0, 0, 0]));
}

#[test]
fn layers_cover_the_ones_below_by_their_alpha() {
    let near = |w: [f32; 4], want: [f32; 4]| w.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6);
    let texel = |a: [u8; 3]| [a[0], a[1], a[2], 255];
    assert!(near(layer_weights(None, 0, 1), [1.0, 0.0, 0.0, 0.0]));
    assert!(near(
        layer_weights(Some(&texel([255, 0, 0])), 0, 2),
        [0.0, 1.0, 0.0, 0.0]
    ));
    assert!(
        near(
            layer_weights(Some(&texel([0, 255, 0])), 0, 2),
            [1.0, 0.0, 0.0, 0.0]
        ),
        "a third layer's alpha is ignored under two layers"
    );
    let w = layer_weights(Some(&texel([51, 102, 204])), 0, 4);
    assert!(near(w, [0.096, 0.024, 0.08, 0.8]));
}

#[test]
fn a_model_is_sorted_by_the_words_in_its_file_name() {
    for (model, want) in [
        ("A\\B\\OakTree01.mdx", Kind::Tree),
        ("A\\B\\SmallBush.mdx", Kind::Shrub),
        ("A\\B\\Boulder03.mdx", Kind::Rock),
        ("A\\B\\WoodenFence.mdx", Kind::Fence),
        ("A\\B\\DustwallowBarrel.mdx", Kind::Prop),
        ("Trees\\Barrel.mdx", Kind::Prop),
        ("A\\ElwynnPine01.mdx", Kind::Tree),
        ("A\\BlastedLandsSpine01.mdx", Kind::Prop),
        ("A\\LampPost.mdx", Kind::Prop),
        ("A\\HumanSignPost01.mdx", Kind::Prop),
        ("A\\ElwynnStoneFence.mdx", Kind::Fence),
        ("A\\LochModanShurb05.mdx", Kind::Shrub),
        ("A\\Stalagtite01.mdl", Kind::Rock),
        ("A\\InnBedCanopy.mdx", Kind::Prop),
    ] {
        assert_eq!(kind(model), want, "{model}");
    }
}

#[test]
fn height_is_bilinear_over_the_outer_grid() {
    let mut c = chunk(0, 0, ZONE);
    for r in 0..9 {
        for k in 0..9 {
            c.positions[r * ROW_STRIDE + k][2] = (r * 10 + k) as f32;
        }
    }
    assert!((height(&c, 0.5, 0.25) - 42.0).abs() < 1e-4);
    assert!((height(&c, 0.0, 0.0)).abs() < 1e-6);
}
