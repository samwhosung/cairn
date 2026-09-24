use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::Collider;
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::collision::GroundDecalSurface;

#[test]
fn only_the_per_foot_keys_plant_a_print() {
    assert_eq!(planted_foot(*b"$FL0"), Some(Foot::Left));
    assert_eq!(planted_foot(*b"$RR1"), Some(Foot::Right));
    assert_eq!(planted_foot(*b"$WL3"), Some(Foot::Left));
    assert_eq!(
        planted_foot(*b"$FSD"),
        None,
        "a footstep's sound plants nothing"
    );
    assert_eq!(planted_foot(*b"$BTH"), None);
    assert_eq!(planted_foot(*b"$FD1"), None);
}

#[test]
fn a_foot_fifty_yards_from_the_eye_prints_and_one_past_it_does_not() {
    let eye = Vec3::new(-9000.0, 80.0, 30.0);
    assert!(!footfall_culls(eye, eye + Vec3::X * 50.0));
    assert!(footfall_culls(eye, eye + Vec3::X * 50.01));
    assert!(
        !footfall_culls(eye, Vec3::NAN),
        "what cannot be compared is kept"
    );
}

fn print_at(spawned: f32) -> Print {
    Print {
        verts: Vec::new(),
        spawned,
        ink: AssetId::default(),
        anchor: Vec3::ZERO,
    }
}

#[test]
fn a_print_holds_at_half_then_fades_out_and_retires_at_six_seconds() {
    assert_eq!(fade(0.0), 127.0 / 255.0);
    assert_eq!(fade(3.0), 127.0 / 255.0);
    assert_eq!(fade(4.5), 63.0 / 255.0);
    assert_eq!(fade(5.988), 0.0);
    assert_eq!(fade(6.0), 0.0);
    let mut prints = Footprints::default();
    prints.add(true, print_at(0.0));
    prints.add(false, print_at(1.0));
    prints.retire(5.99);
    assert_eq!((prints.own.len(), prints.shared.len()), (1, 1));
    prints.retire(6.0);
    assert_eq!((prints.own.len(), prints.shared.len()), (0, 1));
    prints.retire(7.0);
    assert!(prints.shared.is_empty());
}

#[test]
fn each_pool_drops_its_oldest_print_at_its_cap() {
    let mut prints = Footprints::default();
    for i in 0..OWN_CAP + 3 {
        prints.add(true, print_at(i as f32));
    }
    for i in 0..=SHARED_CAP {
        prints.add(false, print_at(i as f32));
    }
    assert_eq!(prints.own.len(), OWN_CAP);
    assert_eq!(prints.own.front().map(|p| p.spawned), Some(3.0));
    assert_eq!(prints.shared.len(), SHARED_CAP);
    assert_eq!(prints.shared.front().map(|p| p.spawned), Some(1.0));
}

fn prints_on_flat_ground(plants: Vec<(Vec3, f32, Foot)>) -> Vec<Vec<EffectVertex>> {
    let mut world = World::new();
    let ground = Collider::trimesh(
        vec![
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    );
    world.spawn((ground, GroundDecalSurface));
    world
        .run_system_once(move |decals: WorldDecal<'_, '_>| {
            plants
                .iter()
                .map(|&(at, yaw, foot)| {
                    project_print(&decals, at, yaw, Vec2::new(0.25, 0.5), foot)
                        .expect("the ground takes it")
                })
                .collect::<Vec<_>>()
        })
        .expect("the system runs")
}

fn extent(verts: &[EffectVertex]) -> (Vec3, Vec3) {
    verts.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), v| (lo.min(v.pos.into()), hi.max(v.pos.into())),
    )
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() < 1e-4
}

#[test]
fn a_print_lies_under_its_foot_along_the_facing_and_a_right_foot_mirrors() {
    let left = Vec3::new(0.0, 0.0, 0.0);
    let right = Vec3::new(0.2, 0.0, -2.33);
    let turned = Vec3::new(3.0, 0.0, 3.0);
    let prints = prints_on_flat_ground(vec![
        (left, 0.0, Foot::Left),
        (right, 0.0, Foot::Right),
        (turned, FRAC_PI_2, Foot::Left),
    ]);
    let (lo, hi) = extent(&prints[0]);
    assert!(
        near(lo, left - Vec3::new(0.25, 0.0, 0.5)) && near(hi, left + Vec3::new(0.25, 0.0, 0.5))
    );
    let (lo, hi) = extent(&prints[1]);
    assert!(
        near((lo + hi) / 2.0, right),
        "each print is where its foot planted"
    );
    let (lo, hi) = extent(&prints[2]);
    assert!(
        near(lo, turned - Vec3::new(0.5, 0.0, 0.25))
            && near(hi, turned + Vec3::new(0.5, 0.0, 0.25)),
        "a quarter turn lays the length across"
    );
    let u_at_least_x = |verts: &[EffectVertex]| {
        verts
            .iter()
            .min_by(|a, b| a.pos[0].total_cmp(&b.pos[0]))
            .map_or(f32::NAN, |v| v.uv[0])
    };
    assert!(u_at_least_x(&prints[0]).abs() < 1e-5);
    assert!(
        (u_at_least_x(&prints[1]) - 1.0).abs() < 1e-5,
        "the right foot mirrors the ink"
    );
}

#[test]
fn only_snow_and_sand_take_prints_and_six_inks_print_them() {
    let Some(data) = std::env::var_os("WOW_DATA") else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let chain = Chain::open(std::path::PathBuf::from(data)).expect("open the chain");
    let mut urls = Vec::new();
    let tables = FootprintTables::read(&chain, |url| {
        urls.push(url);
        Handle::default()
    })
    .expect("the tables read");
    let mut printing: Vec<u32> = tables.printing.iter().copied().collect();
    printing.sort_unstable();
    assert_eq!(printing, [3, 7], "snow and sand");
    assert_eq!(tables.ink.len(), 6);
    assert!(urls.contains(&"mpq://textures/footsteps/basefootprint.blp".to_owned()));
    let snow_effect = tables
        .effect_terrain
        .iter()
        .find_map(|(&e, &t)| (t == 3).then_some(e))
        .expect("a snow ground effect");
    assert!(tables.takes_prints(Underfoot::GroundEffect(snow_effect)));
    assert!(tables.takes_prints(Underfoot::Terrain(7)));
    assert!(
        !tables.takes_prints(Underfoot::Terrain(10)),
        "a building's unauthored floor"
    );
}
