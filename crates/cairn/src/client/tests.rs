use std::f32::consts::PI;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::app::PluginGroupBuilder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::prelude::*;
use bevy::render::ExtractSchedule;
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy::winit::WinitPlugin;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::{UnitShow, UnitSystems};
use world::{CurrentMap, Install, WorldSystems};

use super::assemble;
use crate::args;
use crate::net::{self, Net, OtherPlayer};
use crate::player::PlayerBody;
use crate::player::state::Player;

const FIRST_FRAMES: usize = 3;

/// winit's event loop needs the main thread, which a test does not run on, and pipelined rendering
/// moves the render app onto a thread of its own on cleanup, out of the check's reach.
fn without_a_window(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    plugins
        .disable::<WinitPlugin>()
        .disable::<PipelinedRenderingPlugin>()
}

fn client(argv: &str) -> App {
    assembled(argv).expect("the client assembles")
}

fn assembled(argv: &str) -> Result<App, String> {
    let args = args::parse(argv.split_whitespace().map(str::to_owned)).expect("the arguments");
    let install = Install(Arc::new(mpq::Chain::default()));
    let map = CurrentMap {
        id: 0,
        directory: "Azeroth".into(),
    };
    let mut app = App::new();
    assemble(&mut app, args, &install, map, without_a_window).map_err(|e| e.to_string())?;
    Ok(app)
}

fn schedule_build_failures(app: &mut App) -> Vec<String> {
    app.finish();
    app.cleanup();
    let mut failures = Vec::new();
    for sub_app in app.sub_apps_mut().iter_mut() {
        let world = sub_app.world_mut();
        let labels: Vec<_> = world
            .resource::<Schedules>()
            .iter()
            .map(|(_, schedule)| schedule.label())
            // Its systems read the main world, which only extraction lends the render world: the
            // first frames build it.
            .filter(|label| *label != ExtractSchedule.intern())
            .collect();
        for label in labels {
            world.schedule_scope(label, |world, schedule| {
                if let Err(e) = schedule.initialize(world) {
                    let named = e.to_string(schedule.graph(), world);
                    failures.push(format!("{label:?}: {e}\n{named}"));
                }
            });
        }
    }
    failures
}

#[test]
fn the_client_starts_in_every_mode() {
    for argv in [
        "--mute",
        "--fly --mute",
        "shot --out a.png",
        "shot --display 1 --out a.png",
    ] {
        let mut app = client(argv);
        let failures = schedule_build_failures(&mut app);
        assert!(failures.is_empty(), "`{argv}`: {failures:#?}");
        for _ in 0..FIRST_FRAMES {
            app.update();
        }
        assert_eq!(app.should_exit(), None, "`{argv}`");
    }
}

#[test]
fn an_ordering_cycle_stops_the_client() {
    let mut app = client("shot --out a.png");
    app.configure_sets(Update, WorldSystems.after(UnitSystems));
    let failures = schedule_build_failures(&mut app);
    assert!(
        failures
            .iter()
            .any(|f| f.starts_with("Update") && f.contains("cycle")),
        "{failures:#?}"
    );
}

const WELCOME_WITHIN: Duration = Duration::from_secs(10);

fn running(argv: &str) -> Result<App, String> {
    let mut app = assembled(argv)?;
    app.finish();
    app.cleanup();
    Ok(app)
}

fn frames_until(app: &mut App, done: impl Fn(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + WELCOME_WITHIN;
    while Instant::now() < deadline {
        app.update();
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn welcomed(app: &mut App) -> bool {
    app.world()
        .get_resource::<Net>()
        .and_then(Net::welcome)
        .is_some()
}

fn plays_through_its_server(app: &mut App) -> Result<server::Summary, String> {
    if app.world().get_resource::<Net>().is_none() {
        return Err("no server".into());
    }
    if !frames_until(app, welcomed) {
        return Err("no welcome".into());
    }
    let pose = args::parse(Vec::new()).expect("a bare command").pose;
    let player = app.world().resource::<Player>();
    let placed = (player.pos, player.face_yaw);
    if placed != (wow_to_bevy(pose.target.to_array()), pose.heading) {
        return Err(format!("placed at {placed:?}, not where the camera looks"));
    }
    app.world_mut().resource_mut::<Player>().settling = false;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    frames_until(app, |app| {
        app.world()
            .get_resource::<Net>()
            .is_some_and(|n| n.claims_sent() >= 3)
    });
    let mut net = app.world_mut().resource_mut::<Net>();
    let (claims, corrections) = (net.claims_sent(), net.corrections());
    std::thread::sleep(Duration::from_millis(100));
    let summary = net.stop_hosted().ok_or("not its own server")?;
    let summary = summary.map_err(|e| e.to_string())?;
    if claims < 3 || corrections > 0 || summary.refused.iter().any(|&n| n > 0) {
        return Err(format!(
            "{claims} claims, {corrections} corrections, refused {:?}",
            summary.refused
        ));
    }
    Ok(summary)
}

#[test]
fn a_bare_window_plays_through_a_server_of_its_own_and_a_shot_has_none() {
    let mut window = running("--mute").expect("a window");
    let played = plays_through_its_server(&mut window);
    let mut shot = running("shot --out a.png").expect("a shot");
    let control = plays_through_its_server(&mut shot);
    let played = played.map(|s| s.claims_per_client);
    let control = control.map(|s| s.claims_per_client);
    eprintln!("the window: {played:?}; the shot: {control:?}");
    assert!(played.is_ok_and(|judged_a_second| judged_a_second > 0.0));
    assert_eq!(control, Err("no server".into()));
}

#[test]
fn a_window_serves_itself_the_game_it_names_and_plays_through_it() {
    let rules = server::PHASES.iter().position(|&p| p == "rules");
    let rules_ms = |argv: &str| {
        let mut window = running(argv).expect("a window");
        plays_through_its_server(&mut window).map(|s| rules.map(|p| s.phase_cpu[p]))
    };
    let (game, bare) = (rules_ms("--mute --game melee"), rules_ms("--mute"));
    eprintln!("a tick's rules, with melee: {game:?} ms; without a game: {bare:?} ms");
    assert!(game.is_ok_and(|ms| ms.is_some_and(|ms| ms > 0.0)));
    assert_eq!(bare, Ok(Some(0.0)));
}

#[test]
fn two_windows_alone_never_collide_and_two_hosts_on_one_port_do() {
    let mut alone = ["--mute", "--mute"].map(|argv| running(argv).expect("a window"));
    for app in &mut alone {
        assert!(
            frames_until(app, welcomed),
            "a window alone was not welcomed"
        );
    }
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let first = running(&format!("--mute --host {port}"));
    let second = running(&format!("--mute --host {port}"));
    eprintln!("the second host on {port}: {:?}", second.as_ref().err());
    assert!(first.is_ok() && second.is_err());
}

#[test]
fn a_window_whose_server_fails_says_so_and_plays_on_alone() {
    let mut window = running("--mute").expect("a window");
    let mut failing = net::own_server(None, 0, [0.0; 3], 0.0);
    failing.record = Some(
        std::env::temp_dir()
            .join("no-such-dir-for-a-log")
            .join("inputs.log"),
    );
    let hello = net::hello(
        "Walker".into(),
        &super::character_look(args::Look::default()),
    );
    let net = Net::host(failing, hello).expect("the server starts, and fails in its tick");
    window.insert_resource(net);
    let alone = frames_until(&mut window, |app| {
        app.world().get_resource::<Net>().is_none()
    });
    assert!(alone, "the window still waits on a server that failed");
    assert_eq!(window.should_exit(), None, "the window stopped");
    let mut control = running("--mute").expect("a window");
    assert!(frames_until(&mut control, welcomed));
    for _ in 0..20 {
        control.update();
    }
    assert!(
        control.world().get_resource::<Net>().is_some(),
        "a window whose server runs lost it"
    );
}

const DEATH: u16 = 1;
const DEAD: u16 = 6;
const ATTACK_UNARMED: u16 = 16;

fn one_blow_melee() -> server::Config {
    let over = game::KnobsFile::parse(
        "health = 100\ndamage_min = 100\ndamage_max = 100\nrespawn_s = 3\n",
        "one blow",
    )
    .expect("knobs");
    server::Config {
        game: Some(catalog::load("melee", None, &over.lines, 0).expect("melee")),
        spawns: vec![
            server::Spawn {
                pos: [0.0; 3],
                facing: 0.0,
            },
            server::Spawn {
                pos: [3.0, 0.0, 0.0],
                facing: PI,
            },
        ],
        ..net::own_server(Some(0), 0, [0.0; 3], 0.0)
    }
}

fn own_show(app: &mut App) -> UnitShow {
    let world = app.world_mut();
    let mut own = world.query_filtered::<&UnitShow, With<PlayerBody>>();
    *own.single(world).expect("the player's body")
}

fn the_others_show(app: &mut App) -> Option<UnitShow> {
    let world = app.world_mut();
    let mut others = world.query_filtered::<&UnitShow, With<OtherPlayer>>();
    others.iter(world).next().copied()
}

fn press(app: &mut App, key_code: KeyCode, down: bool) {
    app.world_mut().write_message(KeyboardInput {
        key_code,
        logical_key: Key::Unidentified(NativeKey::Unidentified),
        state: if down {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    });
}

fn stands_across(app: &App) -> [f32; 2] {
    let [x, y, _] = bevy_to_wow(app.world().resource::<Player>().pos);
    [x, y]
}

fn frames_for(app: &mut App, secs: f32) {
    let until = Instant::now() + Duration::from_secs_f32(secs);
    while Instant::now() < until {
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn key_one_swings_and_the_one_it_kills_lies_dead_for_both_and_does_not_walk_while_rooted() {
    let mut host = running("--mute").expect("a window");
    let hello = net::hello("Host".into(), &super::character_look(args::Look::default()));
    let hosting = Net::host(one_blow_melee(), hello).expect("melee is served");
    let addr = hosting.hosted_addr().expect("a port");
    host.insert_resource(hosting);
    let mut guest = running(&format!("--mute --connect {addr}")).expect("a second window");
    for app in [&mut host, &mut guest] {
        assert!(frames_until(app, welcomed), "not welcomed");
        app.world_mut().resource_mut::<Player>().settling = false;
    }
    press(&mut guest, KeyCode::Digit1, true);
    guest.update();
    press(&mut guest, KeyCode::Digit1, false);
    let killed = frames_until(&mut host, |app| {
        app.world().resource::<Player>().rooted && own_show(app).pose == Some(DEAD)
    });
    assert!(
        killed,
        "the host was never killed: {:?}",
        own_show(&mut host)
    );
    let seen_dying = |app: &mut App| {
        own_show(app).play == Some(ATTACK_UNARMED)
            && the_others_show(app)
                == Some(UnitShow {
                    play: Some(DEATH),
                    pose: Some(DEAD),
                })
    };
    assert!(
        frames_until(&mut guest, seen_dying),
        "the guest saw {:?} swing and {:?} die",
        own_show(&mut guest),
        the_others_show(&mut guest)
    );

    let lies = stands_across(&host);
    press(&mut host, KeyCode::KeyW, true);
    frames_for(&mut host, 0.5);
    let tried = stands_across(&host);
    let corrected = host.world().resource::<Net>().corrections();
    assert!(
        (tried[0] - lies[0]).hypot(tried[1] - lies[1]) < 1e-3 && corrected == 0,
        "rooted, it went from {lies:?} to {tried:?} and was corrected {corrected} times"
    );
    host.world_mut().resource_mut::<Player>().rooted = false;
    let caught = frames_until(&mut host, |app| {
        app.world().resource::<Net>().corrections() > 0
    });
    press(&mut host, KeyCode::KeyW, false);
    assert!(
        caught,
        "a client that walks on while rooted is not corrected"
    );

    let risen = frames_until(&mut host, |app| {
        !app.world().resource::<Player>().rooted && own_show(app).pose.is_none()
    });
    let at = stands_across(&host);
    assert!(
        risen && at[0].hypot(at[1]) < 1e-3,
        "risen {risen} at {at:?}"
    );
    assert!(frames_until(&mut guest, |app| {
        the_others_show(app).is_some_and(|s| s.pose.is_none())
    }));
}
