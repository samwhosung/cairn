use super::*;

fn flat_quad(z: f32) -> LiquidMesh {
    LiquidMesh {
        grid: [2, 2],
        wet: vec![true],
        shared: vec![false],
        positions: vec![
            [0.0, 0.0, z],
            [10.0, 0.0, z],
            [0.0, 10.0, z],
            [10.0, 10.0, z],
        ],
        uvs: vec![[0.0, 0.0]; 4],
        depths: vec![1.0; 4],
        indices: vec![0, 1, 2, 1, 3, 2],
        sound_nibble: 0,
        material_id: None,
        kind: LiquidKind::Still,
    }
}

fn grid(
    source: LiquidSource,
    kind: LiquidKind,
    [cols, rows]: [usize; 2],
    step: f32,
    wet: Vec<bool>,
    z: impl Fn(usize, usize) -> f32,
) -> LiquidGrid {
    let positions = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| (i, j)))
        .map(|(i, j)| [i as f32 * step, j as f32 * step, z(i, j)])
        .collect();
    LiquidGrid::new(source, kind, [cols, rows], positions, wet)
}

fn flat(source: LiquidSource, kind: LiquidKind, z: f32) -> LiquidGrid {
    grid(source, kind, [2, 2], 10.0, vec![true], move |_, _| z)
}

fn placement(n: u32) -> Entity {
    Entity::from_raw_u32(n).expect("a valid index")
}

fn room(n: u32) -> WmoRoom {
    WmoRoom {
        instance: placement(n),
        group: 0,
    }
}

fn pool(owner: u32, floor: f32) -> WmoPool {
    WmoPool {
        owner: Some(room(owner)),
        floor,
    }
}

fn wmo(owner: u32, kind: LiquidKind, z: f32) -> LiquidGrid {
    flat(
        LiquidSource::WmoGroup(pool(owner, f32::NEG_INFINITY)),
        kind,
        z,
    )
}

fn inside(owner: u32) -> LiquidClaim {
    LiquidClaim::Inside {
        room: room(owner),
        flooded: None,
    }
}

fn flooded(owner: u32, kind: LiquidKind) -> LiquidClaim {
    LiquidClaim::Inside {
        room: room(owner),
        flooded: Some(kind),
    }
}

#[test]
fn a_flooded_room_is_under_its_liquid_at_every_height() {
    for z in [-9000.0_f32, -125.4, 0.0, 4000.0] {
        let hit = liquid_at(
            std::iter::empty(),
            [10.0, 20.0, z],
            flooded(1, LiquidKind::Still),
        )
        .expect("the room answers with no surface loaded");
        assert_eq!(hit.kind, LiquidKind::Still);
        assert_eq!(hit.surface_z.to_bits(), f32::MAX.to_bits());
    }
}

#[test]
fn a_flooded_room_outranks_a_lower_pool_of_its_own() {
    let sibling = world_grid(
        &flat_quad(-100.0),
        &Transform::IDENTITY,
        LiquidSource::WmoGroup(pool(1, f32::NEG_INFINITY)),
    );
    let feet = [5.0, 5.0, -50.0];
    let plain = liquid_at(std::iter::once(&sibling), feet, inside(1)).expect("the pool");
    assert_eq!(plain.surface_z.to_bits(), (-100.0f32).to_bits());
    let room = liquid_at(
        std::iter::once(&sibling),
        feet,
        flooded(1, LiquidKind::Still),
    )
    .expect("the room");
    assert_eq!(room.surface_z.to_bits(), f32::MAX.to_bits());
}

#[test]
fn an_empty_world_is_dry_for_every_claim() {
    for claim in [inside(1), LiquidClaim::Outdoors, LiquidClaim::Unknown] {
        assert!(liquid_at(std::iter::empty(), [0.0; 3], claim).is_none());
    }
}

#[test]
fn an_identity_footprint_keeps_the_raw_bounds() {
    let g = world_grid(
        &flat_quad(5.0),
        &Transform::IDENTITY,
        LiquidSource::AdtChunk,
    );
    assert_eq!(g.xy_bounds(), Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
    assert_eq!(g.surface_z_at(5.0, 5.0), Some(5.0));
}

#[test]
fn a_yawed_placement_keeps_the_surface_level() {
    let lift = 3.0_f32;
    for deg in [0.0_f32, 30.0, 90.0, 200.0, 355.0] {
        let transform = Transform {
            translation: Vec3::new(100.0, lift, -50.0),
            rotation: Quat::from_rotation_y(deg.to_radians()),
            scale: Vec3::ONE,
        };
        let g = world_grid(&flat_quad(5.0), &transform, LiquidSource::AdtChunk);
        let centre = bevy_to_wow(transform.transform_point(wow_to_bevy([5.0, 5.0, 5.0])));
        let z = g
            .surface_z_at(centre[0], centre[1])
            .unwrap_or_else(|| panic!("yaw {deg}: centre {centre:?} off the grid"));
        assert!((z - (5.0 + lift)).abs() < 1e-3, "yaw {deg}: {z}");
    }
}

#[test]
fn a_rotated_grid_inverts_to_its_cell() {
    let (c, sn) = (0.3f32.cos(), 0.3f32.sin());
    let positions = (0..3)
        .flat_map(|j| (0..3).map(move |i| (i as f32 * 4.0, j as f32 * 4.0)))
        .map(|(a, b)| [100.0 + a * c - b * sn, 50.0 + a * sn + b * c, a])
        .collect();
    let g = LiquidGrid::new(
        LiquidSource::AdtChunk,
        LiquidKind::Magma,
        [3, 3],
        positions,
        vec![true; 4],
    );
    let (a, b) = (6.0f32, 2.0f32);
    let (x, y) = (100.0 + a * c - b * sn, 50.0 + a * sn + b * c);
    assert!((g.surface_z_at(x, y).expect("wet") - 6.0).abs() < 1e-3);
}

#[test]
fn the_surface_is_the_bilinear_of_its_cell() {
    let g = grid(
        LiquidSource::AdtChunk,
        LiquidKind::Still,
        [2, 2],
        10.0,
        vec![true],
        |i, j| match (i, j) {
            (0, 0) => 0.0,
            (1, 0) => 4.0,
            (0, 1) => 2.0,
            _ => 8.0,
        },
    );
    for (x, y, want) in [
        (0.0, 0.0, 0.0),
        (10.0, 0.0, 4.0),
        (0.0, 10.0, 2.0),
        (10.0, 10.0, 8.0),
        (5.0, 0.0, 2.0),
        (5.0, 5.0, 3.5),
        (2.5, 7.5, 2.875),
    ] {
        let got = g.surface_z_at(x, y).expect("wet");
        assert!((got - want).abs() < 1e-4, "({x}, {y}): {got} not {want}");
    }
}

#[test]
fn indoors_and_outdoors_see_different_liquid() {
    let lake = flat(LiquidSource::AdtChunk, LiquidKind::Still, 50.0);
    let canal = wmo(1, LiquidKind::Still, 8.0);
    let all = [&lake, &canal];
    let under = [5.0, 5.0, 0.0];
    let z = |claim| liquid_at(all.into_iter(), under, claim).map(|h| h.surface_z);
    assert_eq!(z(inside(1)), Some(8.0));
    assert_eq!(z(LiquidClaim::Outdoors), Some(50.0));
    assert!(z(LiquidClaim::Unknown).is_some());
    assert!(liquid_at(all.into_iter(), [99.0, 99.0, 0.0], LiquidClaim::Outdoors).is_none());
}

#[test]
fn another_buildings_pool_never_claims_you() {
    let mine = wmo(1, LiquidKind::Still, 8.0);
    let theirs = wmo(2, LiquidKind::Still, 190.0);
    let all = [&mine, &theirs];
    let feet = [5.0, 5.0, 0.0];
    let z = |claim| liquid_at(all.into_iter(), feet, claim).map(|h| h.surface_z);
    assert_eq!(z(inside(1)), Some(8.0));
    assert_eq!(z(inside(2)), Some(190.0));
    assert!(liquid_at([&theirs].into_iter(), feet, inside(1)).is_none());
}

#[test]
fn a_pool_upstairs_does_not_claim_the_room_below() {
    let upstairs = flat(
        LiquidSource::WmoGroup(pool(1, 48.0)),
        LiquidKind::Slime,
        51.98,
    );
    let downstairs = flat(
        LiquidSource::WmoGroup(pool(1, -70.0)),
        LiquidKind::Slime,
        -64.48,
    );
    assert!(!upstairs.answers(inside(1), -63.59));
    assert!(downstairs.answers(inside(1), -63.59));
    let hit = liquid_at(
        [&upstairs, &downstairs].into_iter(),
        [5.0, 5.0, -66.0],
        inside(1),
    );
    assert_eq!(hit.map(|h| h.surface_z), Some(-64.48));
    assert!(upstairs.answers(inside(1), 50.0));
}

#[test]
fn the_floor_holds_for_an_unknown_claim() {
    let upstairs = flat(
        LiquidSource::WmoGroup(pool(1, 48.0)),
        LiquidKind::Slime,
        51.98,
    );
    let at = |z| liquid_at([&upstairs].into_iter(), [5.0, 5.0, z], LiquidClaim::Unknown);
    assert!(at(-63.59).is_none());
    assert!(at(50.0).is_some());
}

#[test]
fn a_group_with_no_box_has_no_floor() {
    let missing = WmoGroupNav {
        flags: 0,
        wmo_group_id: 0,
        bbox_min: [f32::INFINITY; 3],
        bbox_max: [f32::NEG_INFINITY; 3],
        ref_start: 0,
        ref_count: 0,
        interior: true,
        flooded: None,
        fog_indices: [0; 4],
    };
    for nav in [None, Some(&missing)] {
        let unbounded = WmoPool::new(Some(room(1)), &Transform::IDENTITY, nav);
        assert_eq!(unbounded.floor, f32::NEG_INFINITY);
    }
    let g = flat(
        LiquidSource::WmoGroup(WmoPool::new(Some(room(1)), &Transform::IDENTITY, None)),
        LiquidKind::Still,
        8.0,
    );
    assert!(liquid_at([&g].into_iter(), [5.0, 5.0, -9999.0], inside(1)).is_some());
}

#[test]
fn the_floor_follows_the_placement() {
    let nav = WmoGroupNav {
        flags: 0,
        wmo_group_id: 0,
        bbox_min: [-10.0, -10.0, 0.0],
        bbox_max: [10.0, 10.0, 4.0],
        ref_start: 0,
        ref_count: 0,
        interior: true,
        flooded: None,
        fog_indices: [0; 4],
    };
    let lifted = WmoPool::new(
        None,
        &Transform::from_translation(Vec3::new(0.0, 100.0, 0.0)),
        Some(&nav),
    );
    assert!((lifted.floor - 100.0).abs() < 1e-3, "{}", lifted.floor);
    let rolled = WmoPool::new(
        None,
        &Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
        Some(&nav),
    );
    assert!(rolled.floor < -9.0, "{}", rolled.floor);
}

#[test]
fn an_unowned_pool_answers_only_an_unknown_claim() {
    let orphan = flat(
        LiquidSource::WmoGroup(WmoPool {
            owner: None,
            floor: f32::NEG_INFINITY,
        }),
        LiquidKind::Still,
        8.0,
    );
    let feet = [5.0, 5.0, 0.0];
    assert!(liquid_at([&orphan].into_iter(), feet, inside(1)).is_none());
    assert!(liquid_at([&orphan].into_iter(), feet, LiquidClaim::Outdoors).is_none());
    assert!(liquid_at([&orphan].into_iter(), feet, LiquidClaim::Unknown).is_some());
}

#[test]
fn an_indoor_eye_does_not_see_the_terrain_water_overhead() {
    let lake = flat(LiquidSource::AdtChunk, LiquidKind::Still, 32.93);
    let eye = [5.0, 5.0, -62.26];
    assert!(liquid_at([&lake].into_iter(), eye, LiquidClaim::Outdoors).is_some());
    assert!(liquid_at([&lake].into_iter(), eye, inside(1)).is_none());
    assert_eq!(
        submersion_claim_at([&lake].into_iter(), eye, inside(1)),
        None
    );
}

#[test]
fn stacked_surfaces_take_the_lowest() {
    let upper = wmo(1, LiquidKind::Still, 40.0);
    let lower = wmo(1, LiquidKind::Still, 4.0);
    for order in [[&upper, &lower], [&lower, &upper]] {
        let hit = liquid_at(order.into_iter(), [5.0, 5.0, 0.0], inside(1)).expect("wet");
        assert_eq!(hit.surface_z, 4.0);
    }
}

#[test]
fn magma_is_swum_but_is_not_water() {
    let lava = wmo(1, LiquidKind::Magma, 6.0);
    let here = [5.0, 5.0, 0.0];
    let hit = liquid_at([&lava].into_iter(), here, inside(1)).expect("a swim volume");
    assert_eq!(hit.kind, LiquidKind::Magma);
    assert!(water_surface_at([&lava].into_iter(), here, inside(1)).is_none());
}

#[test]
fn a_dry_cell_inside_the_box_is_dry() {
    let g = grid(
        LiquidSource::WmoGroup(pool(1, f32::NEG_INFINITY)),
        LiquidKind::Still,
        [4, 2],
        10.0,
        vec![true, false, true],
        |_, _| 5.0,
    );
    assert!(g.contains(15.0, 5.0));
    assert!(g.surface_z_at(5.0, 5.0).is_some() && g.surface_z_at(25.0, 5.0).is_some());
    assert!(g.surface_z_at(15.0, 5.0).is_none());
    assert!(liquid_at([&g].into_iter(), [15.0, 5.0, 0.0], inside(1)).is_none());
}

#[test]
fn a_degenerate_grid_falls_back_to_its_box() {
    let g = LiquidGrid::new(
        LiquidSource::AdtChunk,
        LiquidKind::Still,
        [2, 2],
        vec![
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 10.0],
            [0.0, 10.0, 0.0],
            [0.0, 10.0, 10.0],
        ],
        vec![true],
    );
    assert_eq!(g.surface_z_at(0.0, 5.0), Some(10.0));
    assert_eq!(g.surface_z_at(50.0, 5.0), None);
}

#[test]
fn a_malformed_grid_claims_and_walks_nothing() {
    let g = LiquidGrid::new(
        LiquidSource::AdtChunk,
        LiquidKind::Still,
        [9, 9],
        vec![[0.0, 0.0, 5.0]; 4],
        vec![true; 64],
    );
    assert_eq!(g.surface_z_at(0.0, 0.0), None);
    assert!(g.xy_bounds().is_none());
    let mut cells = 0;
    g.for_each_wet_cell(|_| cells += 1);
    assert_eq!(cells, 0);
}

#[test]
fn the_eye_margin_is_waters_alone() {
    let water = wmo(1, LiquidKind::Still, 5.0);
    let lava = wmo(1, LiquidKind::Magma, 5.0);
    let eye = [5.0, 5.0, 5.005];
    assert_eq!(
        submersion_claim_at([&water].into_iter(), eye, inside(1)),
        Some((Submersion::Water, 5.0))
    );
    assert_eq!(
        submersion_claim_at([&lava].into_iter(), eye, inside(1)),
        None
    );
    let ocean = flat(LiquidSource::AdtChunk, LiquidKind::Ocean, 0.0);
    assert_eq!(
        submersion_claim_at(
            [&ocean].into_iter(),
            [5.0, 5.0, -2.0],
            LiquidClaim::Outdoors
        ),
        Some((Submersion::Ocean, 0.0))
    );
}

#[test]
fn the_nearest_point_is_on_the_surface_at_the_clamped_column() {
    let mut g = grid(
        LiquidSource::AdtChunk,
        LiquidKind::Still,
        [3, 3],
        10.0,
        vec![true, false, true, true],
        |i, _| i as f32,
    );
    g.sound_nibble = 5;
    assert_eq!(g.sound_nibble(), 5);
    assert_eq!(g.nearest_point(-5.0, 5.0), Some([0.0, 5.0, 0.0]));
    let over_dry = g.nearest_point(15.0, 5.0).expect("wet cells");
    assert_eq!(over_dry, [15.0, 5.0, 2.0]);
    let none = LiquidGrid::new(
        LiquidSource::AdtChunk,
        LiquidKind::Still,
        [2, 2],
        vec![[0.0; 3]; 4],
        vec![false],
    );
    assert!(none.nearest_point(0.0, 0.0).is_none());
}
