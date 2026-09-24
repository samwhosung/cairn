use super::*;

fn cloud(mode: DriftMode) -> DriftCloud {
    let mut c = DriftCloud::default();
    c.scatter(mode);
    c
}

fn in_box(v: Vec3) -> bool {
    [v.x, v.y, v.z]
        .iter()
        .all(|c| *c >= -BOX_HALF && *c < BOX_HALF)
}

#[test]
fn a_scatter_fills_the_box_and_the_edge_spread() {
    for (mode, base) in [
        (DriftMode::Water, SCALE_WATER),
        (DriftMode::Magma, SCALE_MAGMA),
    ] {
        let c = cloud(mode);
        assert_eq!(c.motes.len(), COUNT);
        for m in &c.motes {
            assert!(in_box(m.pos), "{:?}", m.pos);
            assert!((base * 0.5..base * 1.5).contains(&m.edge), "{}", m.edge);
        }
        let mean: Vec3 = c.motes.iter().map(|m| m.pos).sum::<Vec3>() / COUNT as f32;
        assert!(mean.length() < 1.0, "{mean:?}");
    }
}

#[test]
fn the_wrap_keeps_every_mote_in_the_box() {
    let mut c = cloud(DriftMode::Water);
    let mut eye = Vec3::ZERO;
    for _ in 0..500 {
        eye += Vec3::new(0.4, 0.05, -0.3);
        c.advect(DriftMode::Water, eye, 1.0 / 60.0);
        assert!(c.motes.iter().all(|m| in_box(m.pos)));
    }
}

#[test]
fn the_field_stands_still_in_the_world_but_for_the_gust() {
    let mut c = cloud(DriftMode::Water);
    c.gust_amp = 0.0;
    c.gust_freq = 0.0;
    let before: Vec<Vec3> = c.motes.iter().map(|m| m.pos).collect();
    let eye = Vec3::new(3.0, -1.0, 2.0);
    c.advect(DriftMode::Water, eye, 1.0 / 60.0);
    let mut checked = 0;
    for (m, was) in c.motes.iter().zip(&before) {
        let expect = *was - eye;
        if in_box(expect) {
            assert!((m.pos - expect).length() < 1e-3, "{:?} {expect:?}", m.pos);
            checked += 1;
        }
    }
    assert!(checked > COUNT / 2);
}

#[test]
fn a_jump_past_the_box_scatters_the_field() {
    let mut c = cloud(DriftMode::Water);
    c.gust_amp = 0.0;
    let before: Vec<Vec3> = c.motes.iter().map(|m| m.pos).collect();
    c.advect(
        DriftMode::Water,
        Vec3::new(0.0, 0.0, TELEPORT + 1.0),
        1.0 / 60.0,
    );
    assert!(c.motes.iter().all(|m| in_box(m.pos)));
    let moved = c
        .motes
        .iter()
        .zip(&before)
        .filter(|(m, b)| (m.pos - **b).length() > 1e-3)
        .count();
    assert!(moved > COUNT * 9 / 10, "{moved}");
}

#[test]
fn the_gust_never_blows_down_and_leans_level_without_a_bound() {
    let mut c = DriftCloud::default();
    let mut elev = Vec::new();
    for _ in 0..20_000 {
        c.roll_gust();
        let d = c.gust_dir;
        assert!((d.length() - 1.0).abs() < 1e-4);
        assert!(d.y >= 0.0, "{d:?}");
        elev.push(d.y.clamp(-1.0, 1.0).asin().to_degrees());
    }
    elev.sort_by(f32::total_cmp);
    let median = elev[elev.len() / 2];
    let steep = elev.iter().filter(|e| **e > 45.0).count() as f32 / elev.len() as f32;
    assert!(
        (median - GUST_RISE.atan().to_degrees()).abs() < 1.0,
        "{median}"
    );
    assert!((steep - 0.156).abs() < 0.02, "{steep}");
    assert!(elev[elev.len() - 1] > 80.0);
}

#[test]
fn a_gust_lasts_twenty_to_forty_seconds() {
    let mut c = DriftCloud::default();
    for _ in 0..2_000 {
        c.roll_gust();
        let period = 0.5 / c.gust_freq;
        assert!((20.0..=40.0).contains(&period), "{period}");
        assert!((0.005..0.01).contains(&c.gust_amp), "{}", c.gust_amp);
    }
}

#[test]
fn the_gust_drifts_as_far_at_any_frame_rate() {
    let travel = |steps: u32, dt: f32| {
        let mut c = DriftCloud::default();
        c.roll_gust();
        let dir = c.gust_dir;
        (0..steps)
            .map(|_| c.gust(DriftMode::Water, dt).dot(dir))
            .sum::<f32>()
    };
    let (at30, at120) = (travel(30, 1.0 / 30.0), travel(120, 1.0 / 120.0));
    assert!(
        (at30 - at120).abs() / at30.abs().max(1e-6) < 0.05,
        "{at30} {at120}"
    );
}

#[test]
fn magma_sinks_at_a_speed() {
    let mut c = DriftCloud::default();
    let a = c.gust(DriftMode::Magma, 1.0 / 30.0) * 30.0;
    let b = c.gust(DriftMode::Magma, 1.0 / 120.0) * 120.0;
    assert!((a.y - MAGMA_SINK).abs() < 1e-5 && (b.y - MAGMA_SINK).abs() < 1e-5);
    assert!(a.x.abs() < f32::EPSILON && a.z.abs() < f32::EPSILON);
}

#[test]
fn every_atlas_cell_is_its_own_tile() {
    let mut seen = std::collections::HashSet::new();
    for (col, row) in ATLAS {
        assert!((col + 1.0) * CELL <= 1.0 + 1e-6 && (row + 1.0) * CELL <= 1.0 + 1e-6);
        assert!(seen.insert((col as u32, row as u32)));
    }
    assert!(CELLS_MAGMA.contains(&12));
}

#[test]
fn the_cone_never_drops_a_mote_on_screen() {
    let fov = crate::view::FOV_Y;
    for aspect in [4.0 / 3.0, 16.0 / 10.0, 16.0 / 9.0, 21.0 / 9.0, 32.0 / 9.0] {
        let (tx, ty) = cull_limits(fov, aspect);
        assert!(tx >= (fov * 0.5).tan() * aspect && ty >= (fov * 0.5).tan());
    }
    assert_eq!(cull_limits(fov, 16.0 / 9.0), (1.0, 1.0));
}

#[test]
fn a_field_draws_whole_quads_within_the_cap() {
    let c = cloud(DriftMode::Water);
    let cam = Transform::default();
    let mesh = quads(&c, DriftMode::Water, &cam, (1.0, 1.0));
    let n = mesh.count_vertices();
    assert!(n > 0 && n.is_multiple_of(4) && n / 4 <= SUBMIT_CAP, "{n}");
}
