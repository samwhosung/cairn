//! The walk from Northshire Abbey through the vineyard and Goldshire's inn into Crystal Lake, heard:
//! the window's plugins drawn headless, the body steered along the route by scripted keys at a
//! fixed step, and the sound rendered offline. Into the directory `CAIRN_SOUND_WALK` names go the
//! mix as a WAV, every play as a JSON line, and every frame's answers the sound was given (where
//! the listener stands, the area and building, the eye's liquid, the event keys fired, and for each
//! sounding body its pose, room, water and surface) as a JSON line each.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bevy::camera::RenderTarget;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::time::TimeUpdateStrategy;
use sound::{AudioListener, SoundBody};
use world::collision::{CollisionPlugin, LiquidClaim, Liquids};
use world::coords::bevy_to_wow;
use world::doodad_sound::SoundHost;
use world::interior::{
    CurrentArea, CurrentAreaInterior, CurrentWmoInterior, UnitRoom, WmoInteriorKeys,
};
use world::rig_events::AnimEvent;
use world::submersion::Underwater;
use world::surface::{SurfaceUnderfoot, Underfoot};
use world::unit::{BodyDressed, CharacterLook, CharacterTables, UnitBody};
use world::{CurrentMap, Install, Residency, TimeOfDay, WorldCamera};

use crate::player::camera::CameraControl;
use crate::player::state::Player;
use crate::player::{Mode, PlayerBody, PlayerPlugin};
use crate::shot::{Pipelines, headless_plugins, watch_pipelines};
use crate::view::Pose;

const STEP: Duration = Duration::from_nanos(16_666_667);
const SIZE: UVec2 = UVec2::new(1280, 720);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// The liquid loops' reach, which the frame's liquid answer is asked at.
const LIQUID_REACH: f32 = 9.0;

/// Where the walk begins: the abbey's nave, on its floor.
const START: [f32; 3] = [-8908.6, -190.5, 82.5];

/// The route in WoW `(x, y)`, each point with how near counts as reached and how long to stand
/// there, seconds.
const ROUTE: [([f32; 2], f32, f32); 42] = [
    ([-8911.0, -170.0], 1.0, 0.0),
    ([-8912.0, -150.0], 1.0, 0.0),
    ([-8913.0, -141.0], 1.0, 0.0),
    ([-8913.5, -134.0], 1.0, 0.0),
    ([-8922.0, -127.0], 1.5, 0.0),
    ([-8950.0, -120.0], 2.0, 0.0),
    ([-8962.0, -160.0], 2.0, 0.0),
    ([-8970.0, -250.0], 2.0, 0.0),
    ([-8978.0, -288.5], 1.5, 0.0),
    ([-8995.0, -310.5], 1.5, 0.0),
    ([-9012.0, -320.0], 2.0, 0.0),
    ([-8995.0, -310.5], 1.5, 0.0),
    ([-8978.0, -288.5], 1.5, 0.0),
    ([-8985.0, -200.0], 2.0, 0.0),
    ([-8988.0, -100.0], 2.0, 0.0),
    ([-9005.0, -92.0], 2.0, 0.0),
    ([-9040.0, -95.0], 2.0, 0.0),
    ([-9050.0, -70.0], 2.0, 0.0),
    ([-9050.0, -43.0], 1.5, 0.0),
    ([-9062.0, -43.0], 1.5, 0.0),
    ([-9075.0, -45.0], 1.5, 0.0),
    ([-9085.0, -50.0], 2.0, 0.0),
    ([-9116.0, -71.0], 2.0, 0.0),
    ([-9158.0, -102.0], 2.0, 0.0),
    ([-9179.1, -116.0], 2.0, 0.0),
    ([-9234.7, -105.6], 2.0, 0.0),
    ([-9276.3, -70.9], 2.0, 0.0),
    ([-9331.9, -53.5], 2.0, 0.0),
    ([-9373.6, -15.3], 2.0, 0.0),
    ([-9415.2, 33.3], 2.0, 0.0),
    ([-9443.0, 61.1], 2.0, 0.0),
    ([-9455.0, 50.0], 1.0, 0.0),
    ([-9459.5, 44.5], 1.0, 0.0),
    ([-9463.0, 36.0], 1.0, 4.0),
    ([-9459.5, 44.5], 1.0, 0.0),
    ([-9452.0, 52.0], 1.0, 0.0),
    ([-9440.0, 30.0], 2.0, 0.0),
    ([-9440.0, -40.0], 2.0, 0.0),
    ([-9440.0, -75.0], 2.0, 0.0),
    ([-9440.0, -110.0], 2.0, 0.0),
    ([-9440.0, -140.0], 2.0, 0.0),
    ([-9440.0, -150.0], 2.0, 0.0),
];

/// The frame's answers, one JSON line a frame.
#[derive(Resource)]
struct Tape {
    out: std::io::BufWriter<std::fs::File>,
    frame: u64,
}

fn keys(k: WmoInteriorKeys) -> String {
    format!("[{},{},{}]", k.wmo_id, k.name_set, k.group_area_id)
}

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> String) -> String {
    v.map_or_else(|| "null".to_owned(), f)
}

fn vec(v: Vec3) -> String {
    format!("[{},{},{}]", v.x, v.y, v.z)
}

fn claim_of(room: Option<&UnitRoom>) -> LiquidClaim {
    match room.map(UnitRoom::room) {
        Some(Some(_)) => LiquidClaim::Inside,
        Some(None) => LiquidClaim::Outdoors,
        None => LiquidClaim::Unknown,
    }
}

type Heard<'a> = (
    Entity,
    Ref<'a, Transform>,
    &'a GlobalTransform,
    Ref<'a, SoundBody>,
    Option<&'a UnitRoom>,
    Has<sound::Listening>,
);

type Place<'a> = (
    Res<'a, CurrentArea>,
    Res<'a, CurrentAreaInterior>,
    Res<'a, CurrentWmoInterior>,
    Res<'a, Underwater>,
    Res<'a, TimeOfDay>,
);

/// One sounding body as the sound saw it: where its feet were last frame and are now, what it
/// is, its room, the water over both, and the surface under the first.
fn body_line(
    (entity, transform, global, body, room, listening): (
        Entity,
        Ref<'_, Transform>,
        &GlobalTransform,
        Ref<'_, SoundBody>,
        Option<&UnitRoom>,
        bool,
    ),
    liquids: &Liquids<'_, '_>,
    surface: &SurfaceUnderfoot<'_, '_>,
) -> String {
    let feet = global.translation();
    let claim = claim_of(room);
    let water_global = liquids.water_surface_at(bevy_to_wow(feet), claim);
    let water_local = liquids.water_surface_at(bevy_to_wow(transform.translation), claim);
    let under = match surface.at(room.and_then(UnitRoom::room), feet) {
        Some(Underfoot::Terrain(t)) => format!(r#"{{"t":{t}}}"#),
        Some(Underfoot::GroundEffect(g)) => format!(r#"{{"g":{g}}}"#),
        None => "null".to_owned(),
    };
    let room = match room.map(UnitRoom::room) {
        None => "null".to_owned(),
        Some(None) => r#""out""#.to_owned(),
        Some(Some(r)) => format!("[{},{}]", r.instance.to_bits(), r.group),
    };
    format!(
        r#"{{"e":{},"g":{},"l":{},"moved":{},"new":{},"display":{},"h":{},"wade":{},"room":{},"wg":{},"wl":{},"under":{},"me":{}}}"#,
        entity.to_bits(),
        vec(feet),
        vec(transform.translation),
        transform.is_changed(),
        body.is_changed(),
        body.display,
        body.collision_height,
        body.wade_max,
        room,
        opt(water_global, |w| w.to_string()),
        opt(water_local, |w| w.to_string()),
        under,
        listening,
    )
}

#[allow(clippy::too_many_arguments)]
fn record(
    mut tape: ResMut<'_, Tape>,
    time: Res<'_, Time>,
    listener: Res<'_, AudioListener>,
    place: Place<'_>,
    mut events: MessageReader<'_, '_, AnimEvent>,
    mut gone: RemovedComponents<'_, '_, SoundHost>,
    bodies: Query<'_, '_, Heard<'_>>,
    liquids: Liquids<'_, '_>,
    surface: SurfaceUnderfoot<'_, '_>,
) {
    let (area, area_interior, interior, underwater, minute) = place;
    let mut line = String::new();
    let _ = write!(
        line,
        r#"{{"f":{},"t":{},"dt":{},"lis":[{},{},{},{},{},{},{}],"area":{},"ai":{},"wi":{},"sub":"{:?}","min":{}"#,
        tape.frame,
        time.elapsed_secs_f64(),
        time.delta_secs(),
        listener.pos.x,
        listener.pos.y,
        listener.pos.z,
        listener.rot.x,
        listener.rot.y,
        listener.rot.z,
        listener.rot.w,
        opt(area.0, |a| a.to_string()),
        opt(area_interior.0, keys),
        opt(interior.0, keys),
        underwater.0,
        minute.minute,
    );
    let events: Vec<String> = events
        .read()
        .map(|e| {
            format!(
                r#"[{},"{}",{},{},{}]"#,
                e.entity.to_bits(),
                String::from_utf8_lossy(&e.ident),
                e.data,
                e.anim_id,
                vec(e.pos)
            )
        })
        .collect();
    let gone: Vec<String> = gone.read().map(|e| e.to_bits().to_string()).collect();
    let _ = write!(
        line,
        r#","ev":[{}],"gone":[{}],"bodies":["#,
        events.join(","),
        gone.join(",")
    );
    let mut listening_at = None;
    let lines: Vec<String> = bodies
        .iter()
        .map(|b| {
            if b.5 {
                listening_at = Some(b.1.translation);
            }
            body_line(b, &liquids, &surface)
        })
        .collect();
    line.push_str(&lines.join(","));
    line.push(']');
    let liquid = listening_at.map(|at| {
        let near = liquids.nearest_per_class(bevy_to_wow(at), LIQUID_REACH);
        let near: Vec<String> = near
            .iter()
            .map(|n| {
                opt(n.as_ref(), |n| {
                    format!(
                        "[{},{},{},{},{}]",
                        n.dist_sq, n.point[0], n.point[1], n.point[2], n.nibble
                    )
                })
            })
            .collect();
        format!(r#"{{"at":{},"near":[{}]}}"#, vec(at), near.join(","))
    });
    let _ = write!(
        line,
        r#","liq":{}}}"#,
        liquid.unwrap_or_else(|| "null".to_owned())
    );
    tape.frame += 1;
    let _ = writeln!(tape.out, "{line}");
}

struct Walk {
    app: App,
}

impl Walk {
    fn new() -> Option<Self> {
        let (Some(data), Some(out)) = (
            std::env::var_os("WOW_DATA"),
            std::env::var_os("CAIRN_SOUND_WALK"),
        ) else {
            eprintln!("skipped: set WOW_DATA and CAIRN_SOUND_WALK");
            return None;
        };
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).expect("the output directory");
        let install = Install::open(&PathBuf::from(data)).expect("open the install");
        let map = CurrentMap::find(&install.0, "Azeroth").expect("the map");
        let tables = CharacterTables::load(&install).expect("the character tables");
        let tape = std::fs::File::create(out.join("tape.jsonl")).expect("the tape");
        let mut app = App::new();
        world::register_source(&mut app, &install);
        app.add_plugins(headless_plugins())
            .insert_resource(map)
            .insert_resource(tables)
            .insert_resource(TimeOfDay { minute: 12 * 60 })
            .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
            .insert_resource(Tape {
                out: std::io::BufWriter::new(tape),
                frame: 0,
            })
            .add_plugins((
                CollisionPlugin,
                PlayerPlugin {
                    pose: Pose::orbit(Vec3::from_array(START), 90.0, 12.0, 16.0),
                    mode: Mode::Walk,
                    look: CharacterLook::naked(1, 0),
                },
                world::LoadersPlugin,
                world::WorldPlugin,
                sound::SoundPlugin {
                    output: sound::Output::Offline {
                        sample_rate: sound::OFFLINE_SAMPLE_RATE,
                    },
                    mix_tap: None,
                    record: Some(out.join("cairn.wav")),
                    log: Some(out.join("cairn-plays.jsonl")),
                },
            ))
            .add_systems(Update, record.after(sound::SoundSystems));
        let pipelines = watch_pipelines(&mut app);
        app.insert_resource(pipelines);
        app.finish();
        app.cleanup();
        let target =
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(
                    SIZE.x,
                    SIZE.y,
                    TextureFormat::Rgba8UnormSrgb,
                    None,
                ));
        app.update();
        let camera = app
            .world_mut()
            .query_filtered::<Entity, With<WorldCamera>>()
            .single(app.world())
            .expect("the follow camera");
        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::Image(target.into()));
        let mut walk = Self { app };
        walk.settle();
        Some(walk)
    }

    fn settle(&mut self) {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while !self.arrived() {
            assert!(Instant::now() < deadline, "the world never arrived");
            self.app.update();
            std::thread::sleep(STEP);
        }
    }

    fn arrived(&mut self) -> bool {
        let world = self.app.world_mut();
        let pipelines = world.resource::<Pipelines>();
        assert!(
            !pipelines.failed.load(Ordering::Relaxed),
            "a render pipeline failed"
        );
        if world.resource::<Player>().settling
            || !world.resource::<Residency>().settled()
            || !pipelines.built.load(Ordering::Relaxed)
        {
            return false;
        }
        world
            .query_filtered::<(), (With<PlayerBody>, With<BodyDressed>, With<UnitBody>)>()
            .single(world)
            .is_ok()
    }

    fn key(&mut self, key_code: KeyCode, state: ButtonState) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn wow(&self) -> [f32; 3] {
        bevy_to_wow(self.app.world().resource::<Player>().pos)
    }

    fn aim(&mut self, heading_deg: f32) {
        self.app.world_mut().resource_mut::<Player>().face_yaw = heading_deg.to_radians();
    }

    fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.app.update();
        }
    }

    /// Steers at each point in turn; a body that stops closing on its point turns aside for a
    /// moment, alternating sides, and tries again.
    fn walk(&mut self) {
        self.key(KeyCode::KeyW, ButtonState::Pressed);
        for (n, &(point, near, pause)) in ROUTE.iter().enumerate() {
            let (mut best, mut since, mut detours) = (f32::MAX, 0usize, 0usize);
            loop {
                let [x, y, z] = self.wow();
                let dist = (point[0] - x).hypot(point[1] - y);
                if dist < near {
                    eprintln!("point {n} ({point:?}) reached at ({x:.1}, {y:.1}, {z:.1})");
                    if pause > 0.0 {
                        self.key(KeyCode::KeyW, ButtonState::Released);
                        self.run((pause / STEP.as_secs_f32()).round() as usize);
                        self.key(KeyCode::KeyW, ButtonState::Pressed);
                    }
                    break;
                }
                if dist < best - 0.2 {
                    (best, since) = (dist, 0);
                } else {
                    since += 1;
                }
                let heading = (point[1] - y).atan2(point[0] - x).to_degrees();
                if since > 90 {
                    detours += 1;
                    assert!(
                        detours <= 12,
                        "stuck short of point {n} ({point:?}) at ({x:.1}, {y:.1}, {z:.1})"
                    );
                    eprintln!("point {n}: stalled at ({x:.1}, {y:.1}, {z:.1}), stepping aside");
                    let side = if detours % 2 == 1 { 70.0 } else { -70.0 };
                    self.aim(heading + side);
                    self.run(45);
                    (best, since) = (f32::MAX, 0);
                    continue;
                }
                self.aim(heading);
                self.run(1);
            }
        }
        self.key(KeyCode::KeyW, ButtonState::Released);
    }

    /// From the surface, the eye zoomed into the head: down, level, and back up, so the eye goes
    /// under and comes out. Whether it went under.
    fn dive(&mut self) -> bool {
        {
            let mut control = self.app.world_mut().resource_mut::<CameraControl>();
            control.distance = 0.0;
            control.target_distance = 0.0;
        }
        self.run(60);
        let mut under = false;
        self.key(KeyCode::KeyW, ButtonState::Pressed);
        for (pitch, secs) in [(-0.7, 2.5), (0.0, 3.0), (0.7, 3.0)] {
            for _ in 0..(secs / STEP.as_secs_f32()).round() as usize {
                self.app.world_mut().resource_mut::<Player>().mover_pitch = pitch;
                self.run(1);
                under |= *self.app.world().resource::<Underwater>() != Underwater::default();
            }
        }
        self.key(KeyCode::KeyW, ButtonState::Released);
        under
    }
}

#[test]
#[ignore = "draws on the GPU and walks for minutes; set WOW_DATA and CAIRN_SOUND_WALK"]
fn the_walk_from_the_abbey_into_crystal_lake_is_heard() {
    let Some(mut walk) = Walk::new() else {
        return;
    };
    let begun = walk.app.world().resource::<Time>().elapsed_secs();
    walk.walk();
    let under = walk.dive();
    walk.run(300);
    let t = walk.app.world().resource::<Time>().elapsed_secs() - begun;
    walk.app
        .world_mut()
        .resource_mut::<Tape>()
        .out
        .flush()
        .expect("the tape flushes");
    eprintln!("walked for {t:.1} s of game time; the eye went under: {under}");
    assert!(walk.wow()[1] < -130.0, "in the lake: {:?}", walk.wow());
    assert!(under, "the eye never went under the lake");
}
