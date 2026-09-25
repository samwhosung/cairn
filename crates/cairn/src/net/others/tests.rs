use std::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use protocol::{Movement, flags};

use super::*;

fn state(flags: u32, pos: [f32; 3], fall_time: u32) -> State {
    State::of(&Movement {
        flags,
        pos,
        facing: 1.0,
        fall_time,
        jump: Jump {
            z_speed: -7.955_547,
            cos: 1.0,
            sin: 0.0,
            xy_speed: 7.0,
        },
        ..Movement::default()
    })
}

#[test]
fn a_move_mid_arc_keeps_the_relayed_launch_and_counts_its_fall_on_the_servers_clock() {
    let e = Entity::PLACEHOLDER;
    let leaping = state(flags::FALLING | flags::FORWARD, [9000.0, -100.0, 50.0], 200);
    let mut r = Relayed::of(e, &leaping, 1000, [9010.0, -90.0, 50.0]);
    assert!((r.wow_pos[0] - 9000.0).abs() < 0.01 && (r.wow_pos[1] + 100.0).abs() < 0.01);
    r.wow_pos = [9003.0, -100.0, 51.0];
    let mv = r.relay_move(1500);
    assert_eq!(
        (mv.fall_time, mv.flags),
        (700, flags::FALLING | flags::FORWARD)
    );
    assert!(mv.jump.is_some_and(|j| (j.xy_speed - 7.0).abs() < 1e-6));
    let landed = Relayed::of(
        e,
        &state(flags::FORWARD, [9004.0, -100.0, 50.0], 0),
        1600,
        [0.0; 3],
    );
    assert_eq!((landed.relay_move(1600).fall_time, landed.jump), (0, None));
}

fn take(app: &mut App, others: &mut Others, record: Record<'_>, server_ms: u32, real_ms: f64) {
    let at = BatchContext {
        server_ms,
        read_around: [100.0, 50.0, 10.0],
        arrived_real_ms: real_ms,
        now_real_ms: real_ms,
        game_secs: app.world().resource::<Time>().elapsed_secs(),
    };
    others.take(&mut app.world_mut().commands(), record, &at);
    app.world_mut().flush();
}

fn remotes(app: &mut App) -> Vec<(OtherPlayer, [f32; 3], u32)> {
    let world = app.world_mut();
    world
        .query::<(&OtherPlayer, &RemoteMotion)>()
        .iter(world)
        .map(|(r, m)| (r.clone(), m.wow_pos, m.flags))
        .collect()
}

#[test]
fn a_player_appears_where_it_stands_moves_as_relayed_and_fades_out_when_it_leaves() {
    let mut app = App::new();
    app.init_resource::<Time>();
    let mut others = Others::default();
    let appearance = Appearance {
        race: 2,
        ..Appearance::default()
    };
    let appear = Record::Appear {
        slot: 3,
        id: 42,
        name: "Anna",
        appearance,
        state: state(0, [101.0, 52.0, 10.0], 0),
    };
    take(&mut app, &mut others, appear, 1000, 0.0);
    let seen = remotes(&mut app);
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0].0.id, seen[0].0.name.as_str(), seen[0].2),
        (42, "Anna", 0)
    );
    assert!((seen[0].1[0] - 101.0).abs() < 0.01 && (seen[0].1[1] - 52.0).abs() < 0.01);
    let running = Record::State {
        slot: 3,
        state: state(flags::FORWARD, [102.0, 52.0, 10.0], 0),
    };
    take(&mut app, &mut others, running, 1050, 50.0);
    let seen = remotes(&mut app);
    assert_eq!(seen[0].2, flags::FORWARD, "due on arrival, so applied");
    assert!((seen[0].1[0] - 102.0).abs() < 0.01);
    take(
        &mut app,
        &mut others,
        Record::Vanish { slot: 3 },
        1100,
        100.0,
    );
    assert!(remotes(&mut app).is_empty(), "no longer a player at once");
    let fading = |app: &mut App| {
        let world = app.world_mut();
        world.query::<&Leaving>().iter(world).count()
    };
    assert_eq!(fading(&mut app), 1, "but its body fades");
    for _ in 0..3 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1000));
        app.world_mut()
            .run_system_once(fade_leaving)
            .expect("the fade runs");
        app.world_mut().flush();
    }
    assert_eq!(fading(&mut app), 0, "and is gone two seconds on");
}
