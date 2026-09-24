//! The cairn client: walks a window through a WoW 1.12.1 install, or renders one shot of it to a PNG.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod args;
mod fixture;
mod fly;
mod player;
mod shot;
mod view;

use std::path::Path;

use bevy::prelude::*;
use world::unit::{BodySkin, CharacterLook, CharacterTables};
use world::{CurrentMap, Install};

/// A debug build aborts on an allocation inside the sound output's realtime scopes.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn main() -> AppExit {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{}", args::USAGE);
        return AppExit::Success;
    }
    let args = match args::parse(argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("cairn: {e}\n\n{}", args::USAGE);
            return AppExit::from_code(2);
        }
    };
    let Some(data) = std::env::var_os("WOW_DATA") else {
        eprintln!("cairn: set WOW_DATA to the Data directory of a WoW 1.12.1 install");
        return AppExit::from_code(2);
    };
    let install = match Install::open(Path::new(&data)) {
        Ok(install) => install,
        Err(e) => {
            eprintln!("cairn: {e}");
            return AppExit::error();
        }
    };
    let map = match CurrentMap::find(&install.0, &args.map) {
        Ok(map) => map,
        Err(e) => {
            eprintln!("cairn: {e}");
            return AppExit::from_code(2);
        }
    };
    let mut app = App::new();
    if args.mode == args::Mode::Window {
        let tables = match CharacterTables::load(&install) {
            Ok(tables) => tables,
            Err(e) => {
                eprintln!("cairn: the install's character tables: {e}");
                return AppExit::error();
            }
        };
        if let Err(e) = check_look_offered(&tables, args.look) {
            eprintln!("cairn: {e}");
            return AppExit::from_code(2);
        }
        app.insert_resource(tables);
    }
    world::register_source(&mut app, &install);
    match args.mode {
        args::Mode::Window => app.add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "cairn".into(),
                    resolution: args.size.into(),
                    ..Window::default()
                }),
                ..WindowPlugin::default()
            }),
            world::collision::CollisionPlugin,
            player::PlayerPlugin {
                pose: args.pose,
                mode: if args.start_flying {
                    player::Mode::Fly
                } else {
                    player::Mode::Walk
                },
                look: character_look(args.look),
            },
            sound_plugin(args.mute),
        )),
        args::Mode::Shot(out) => {
            app.add_plugins((
                shot::headless_plugins(),
                shot::ShotPlugin {
                    pose: args.pose,
                    size: args.size,
                    out,
                    aged_by: match args.display {
                        Some(_) => shot::AgedBy::Subject,
                        None => shot::AgedBy::World {
                            after_loading: args.world_age,
                        },
                    },
                },
            ));
            if let Some(display) = args.display {
                app.add_plugins((
                    world::collision::CollisionPlugin,
                    fixture::FixturePlugin(display),
                ));
            }
            &mut app
        }
    };
    app.insert_resource(map)
        .insert_resource(args.time)
        .insert_resource(world::FullScreenGlow(args.glow))
        .add_plugins((world::LoadersPlugin, world::WorldPlugin))
        .run()
}

fn sound_plugin(mute: bool) -> sound::SoundPlugin {
    sound::SoundPlugin {
        output: if mute {
            sound::Output::Offline {
                sample_rate: sound::OFFLINE_SAMPLE_RATE,
            }
        } else {
            sound::Output::Device
        },
        mix_tap: None,
        record: None,
        log: None,
    }
}

fn character_look(look: args::Look) -> CharacterLook {
    CharacterLook {
        race: look.race,
        sex: look.sex,
        skin: look.skin,
        face: look.face,
        hair_style: look.hair,
        hair_color: look.hair_color,
        facial_hair: look.facial_hair,
        body: BodySkin::Composite,
        equipment: [0; 10],
    }
}

fn check_look_offered(tables: &CharacterTables, look: args::Look) -> Result<(), String> {
    let sex = if look.sex == 0 { "male" } else { "female" };
    let who = format!("a {} {sex}", look.race_name());
    let ranges = tables
        .create
        .ranges(look.race, look.sex)
        .ok_or_else(|| format!("the install offers no {who}"))?;
    for (flag, value, count) in [
        ("skin", look.skin, ranges.skin),
        ("face", look.face, ranges.face),
        ("hair", look.hair, ranges.hair_style),
        ("hair-color", look.hair_color, ranges.hair_color),
        ("facial-hair", look.facial_hair, ranges.facial_hair),
    ] {
        if value >= count.max(1) {
            return Err(format!(
                "--{flag} {value} is not offered to {who}, who has 0 to {}",
                count.saturating_sub(1)
            ));
        }
    }
    Ok(())
}
