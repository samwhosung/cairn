//! Pictures of the walker from its own follow camera: the window's plugins drawn headless into an
//! image, the body walked by scripted keys at a fixed step, and a PNG taken with the clocks held,
//! or the cost of its frames timed. They need a GPU as well as the install, so they run only when
//! asked for, writing into the directory `CAIRN_PICTURES` names.

use bevy::input::ButtonState;
use bevy::prelude::*;
use world::unit::{CharacterLook, UnitShow};

use super::painter::{Painter, frame_costs, rig_census};
use super::walker::Through;
use crate::player::PlayerBody;

const ATTACK_UNARMED: u16 = 16;
const COMBAT_WOUND: u16 = 9;
const FROM_ITS_SIDE: f32 = std::f32::consts::FRAC_PI_2;

pub(super) const GOLDSHIRE: [f32; 2] = [-9439.1, 51.2];
pub(super) const EAST: f32 = 270.0;
const HILLTOP_SOUTH_OF_GOLDSHIRE: [f32; 2] = [-9200.0, -420.0];
const SUN_BEARING: f32 = 45.0;
pub(super) struct Stand {
    pub(super) xy: [f32; 2],
    pub(super) heading: f32,
}

pub(super) const FACING_A_GOLDSHIRE_LAMPPOST: Stand = Stand {
    xy: [-9433.0, 44.0],
    heading: 215.0,
};
const FACING_A_LAMPPOST_BELOW_THE_ABBEY: Stand = Stand {
    xy: [-8952.0, -113.0],
    heading: 320.0,
};
const SOUTH: f32 = 180.0;
const IN_A_TREES_SHADOW_BELOW_THE_ABBEY: Stand = Stand {
    xy: [-8960.0, -133.0],
    heading: SUN_BEARING + 180.0,
};
const IN_THE_SUN_FOUR_YARDS_EAST: [f32; 2] = [
    IN_A_TREES_SHADOW_BELOW_THE_ABBEY.xy[0],
    IN_A_TREES_SHADOW_BELOW_THE_ABBEY.xy[1] - 4.0,
];
pub(super) const ON_THE_SNOW_OUTSIDE_KHARANOS: Stand = Stand {
    xy: [-5650.0, -450.0],
    heading: 0.0,
};
const ON_THE_SAND_OF_THE_WESTFALL_COAST: Stand = Stand {
    xy: [-11350.0, 1850.0],
    heading: 0.0,
};

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_stands_runs_and_jumps_in_goldshire() {
    let Some(mut p) = Painter::new(GOLDSHIRE, EAST, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.wait(2.0);
    p.shoot("goldshire-1-standing");
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.5);
    p.shoot("goldshire-2-running");
    p.key(KeyCode::Space, ButtonState::Pressed);
    p.run(1);
    p.key(KeyCode::Space, ButtonState::Released);
    p.wait(0.25);
    p.shoot("goldshire-3-jumping");
    p.wait(1.0);
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.wait(0.5);
    p.shoot("goldshire-4-landed");
    p.orbit(std::f32::consts::PI, 4.0);
    p.wait(1.0);
    p.shoot("goldshire-5-face");
    p.orbit(0.0, 1.2);
    p.wait(1.0);
    p.shoot("goldshire-6-fading");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_under_the_sky_at_dawn_noon_dusk_and_night() {
    let hill = HILLTOP_SOUTH_OF_GOLDSHIRE;
    let Some(mut p) = Painter::new(hill, SUN_BEARING, CharacterLook::naked(1, 0)) else {
        return;
    };
    for (name, hour, minute, tilt) in [
        ("sky-1-dawn", 6, 30, 0.1),
        ("sky-2-noon", 12, 0, 0.35),
        ("sky-3-dusk", 20, 15, 0.1),
        ("sky-4-night", 0, 30, 0.6),
    ] {
        p.set_time(hour, minute);
        p.tilt_up(tilt);
        p.wait(2.0);
        p.shoot(name);
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_doodads_move_in_goldshire_and_before_the_abbey() {
    for (place, stand) in [
        ("goldshire", FACING_A_GOLDSHIRE_LAMPPOST),
        ("abbey", FACING_A_LAMPPOST_BELOW_THE_ABBEY),
    ] {
        let Some(mut p) = Painter::new(stand.xy, stand.heading, CharacterLook::naked(1, 0)) else {
            return;
        };
        p.orbit(0.0, 8.0);
        p.tilt_up(-0.1);
        p.wait(2.0);
        for i in 0..4 {
            p.shoot(&format!("{place}-doodads-{i}"));
            p.wait(0.4);
        }
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_in_the_sun_in_a_trees_shadow_and_by_a_lamp_at_night() {
    let shade = IN_A_TREES_SHADOW_BELOW_THE_ABBEY;
    let human = CharacterLook::naked(1, 0);
    let Some(mut p) = Painter::new(IN_THE_SUN_FOUR_YARDS_EAST, shade.heading, human.clone()) else {
        return;
    };
    p.wait(2.0);
    p.shoot("light-1-sun");
    p.stand_on(shade.xy);
    p.wait(2.0);
    p.shoot("light-2-shadow");
    let lamp = FACING_A_GOLDSHIRE_LAMPPOST;
    let Some(mut p) = Painter::new(lamp.xy, lamp.heading, human) else {
        return;
    };
    p.orbit(0.0, 6.0);
    p.set_time(0, 30);
    p.wait(2.0);
    p.shoot("light-3-lamp-at-night");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_runs_on_snow_strafes_and_sits() {
    let snow = ON_THE_SNOW_OUTSIDE_KHARANOS;
    let Some(mut p) = Painter::new(snow.xy, snow.heading, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.5);
    p.shoot("pose-1-running-on-snow");
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.key(KeyCode::KeyQ, ButtonState::Pressed);
    p.wait(1.0);
    p.shoot("pose-2-strafing");
    p.key(KeyCode::KeyQ, ButtonState::Released);
    p.wait(1.0);
    p.key(KeyCode::KeyX, ButtonState::Pressed);
    p.run(1);
    p.key(KeyCode::KeyX, ButtonState::Released);
    p.wait(1.5);
    p.shoot("pose-3-sitting");
}

/// Told straight to the body, as no game runs here: the shots come out alike every run.
fn show(p: &mut Painter, anim: u16) {
    let world = p.app.world_mut();
    let mut own = world
        .query_filtered::<&mut UnitShow, With<PlayerBody>>()
        .single_mut(world)
        .expect("the window's body");
    own.play = Some(anim);
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_swings_standing_and_on_the_run() {
    let Some(mut p) = Painter::new(GOLDSHIRE, EAST, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.orbit(FROM_ITS_SIDE, 5.0);
    p.wait(2.0);
    show(&mut p, ATTACK_UNARMED);
    p.wait(0.3);
    p.shoot("swing-1-standing");
    p.wait(1.0);
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.5);
    show(&mut p, ATTACK_UNARMED);
    p.wait(0.3);
    p.orbit(FROM_ITS_SIDE, 5.0);
    p.shoot("swing-2-running");
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.orbit(-FROM_ITS_SIDE, 5.0);
    p.wait(1.5);
    show(&mut p, ATTACK_UNARMED);
    p.wait(0.2);
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(0.35);
    p.orbit(FROM_ITS_SIDE, 5.0);
    p.shoot("swing-3-standing-then-running");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_is_hit_standing_and_runs_on() {
    let Some(mut p) = Painter::new(GOLDSHIRE, EAST, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.orbit(FROM_ITS_SIDE, 5.0);
    p.wait(2.0);
    show(&mut p, COMBAT_WOUND);
    p.wait(0.25);
    p.shoot("wound-1-standing");
    p.orbit(-FROM_ITS_SIDE, 5.0);
    p.wait(1.5);
    show(&mut p, COMBAT_WOUND);
    p.wait(0.15);
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(0.3);
    p.orbit(FROM_ITS_SIDE, 5.0);
    p.shoot("wound-2-running-on");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_leaves_prints_on_snow_and_sand_and_breathes_in_the_cold() {
    // Every stand a dwarf rolls breathes; one of a human's never does.
    let dwarf = CharacterLook::naked(3, 0);
    for (ground, stand, look) in [
        ("snow", ON_THE_SNOW_OUTSIDE_KHARANOS, dwarf),
        (
            "sand",
            ON_THE_SAND_OF_THE_WESTFALL_COAST,
            CharacterLook::naked(1, 0),
        ),
    ] {
        let Some(mut p) = Painter::new(stand.xy, stand.heading, look) else {
            return;
        };
        p.key(KeyCode::KeyW, ButtonState::Pressed);
        p.wait(2.0);
        p.key(KeyCode::KeyW, ButtonState::Released);
        p.orbit(std::f32::consts::PI, 5.0);
        p.tilt_up(-0.8);
        p.wait(0.5);
        p.shoot(&format!("marks-{ground}-prints"));
        if ground == "snow" {
            p.orbit(-std::f32::consts::FRAC_PI_2, 3.0);
            p.tilt_up(-0.1);
            p.wait(2.4);
            p.shoot("marks-snow-breath-1");
            p.wait(0.3);
            p.shoot("marks-snow-breath-2");
        }
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn a_tauren_and_a_gnome_stand_where_the_human_does() {
    for (name, look) in [
        ("race-tauren", CharacterLook::naked(6, 0)),
        ("race-gnome", CharacterLook::naked(7, 1)),
    ] {
        let Some(mut p) = Painter::new(GOLDSHIRE, EAST, look) else {
            return;
        };
        p.wait(2.0);
        p.shoot(name);
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_starts_where_a_bare_window_looks() {
    let pose = crate::args::parse(Vec::new()).expect("a bare command").pose;
    let (feet, heading) = (pose.target.to_array(), pose.heading.to_degrees());
    let look = CharacterLook::naked(1, 0);
    let through = Some(Through::ItsOwn { record: None });
    let Some(mut p) = Painter::build(feet, heading, look, through) else {
        return;
    };
    p.clock().pause();
    p.settle();
    p.clock().unpause();
    p.wait(2.0);
    p.shoot("start-1-standing");
    p.orbit(std::f32::consts::PI, 4.0);
    p.wait(1.0);
    p.shoot("start-2-face");
}

#[test]
#[ignore = "a measurement, for a release build on a GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_frame_cost_of_goldshire() {
    for round in 1..=2 {
        for (served, through) in [
            ("through its own server", Some(Through::ItsOwnInRealTime)),
            ("with no server", None),
        ] {
            let look = CharacterLook::naked(1, 0);
            let Some(mut p) = Painter::standing(GOLDSHIRE, EAST, look, through) else {
                return;
            };
            p.wait(2.0);
            let standing = frame_costs(&mut p, 600);
            let rigs = rig_census(&mut p);
            p.key(KeyCode::KeyW, ButtonState::Pressed);
            let running = frame_costs(&mut p, 1200);
            eprintln!(
                "goldshire {served}, round {round}: standing {standing} with {rigs}; running \
                 {running}"
            );
        }
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_wades_and_swims_into_crystal_lake() {
    let Some(mut p) = Painter::new(super::SHORE, SOUTH, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.wait(2.0);
    p.shoot("lake-1-shore");
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.3);
    p.shoot("lake-2-wading");
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.wait(1.2);
    p.shoot("lake-3-standing-in-the-shallows");
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.5);
    p.shoot("lake-4-swimming");
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.wait(1.2);
    p.shoot("lake-5-floating");
    p.orbit(std::f32::consts::PI, 4.0);
    p.wait(1.0);
    p.shoot("lake-6-floating-close");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_swims_into_a_stormwind_canal() {
    let look = CharacterLook::naked(1, 0);
    let Some(mut p) = Painter::new(super::CANAL_RAMP, super::DOWN_THE_RAMP, look) else {
        return;
    };
    p.wait(2.0);
    p.shoot("canal-1-ramp");
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.0);
    p.shoot("canal-2-wading");
    p.wait(1.0);
    p.shoot("canal-3-swimming");
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.wait(1.2);
    p.shoot("canal-4-floating");
}
