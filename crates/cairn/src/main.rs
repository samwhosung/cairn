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
use world::{CurrentMap, Install};

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
            },
        )),
        args::Mode::Shot(out) => {
            app.add_plugins((
                shot::headless_plugins(),
                shot::ShotPlugin {
                    pose: args.pose,
                    size: args.size,
                    out,
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
        .add_plugins((world::LoadersPlugin, world::WorldPlugin))
        .run()
}
