//! The cairn client: flies a window over a WoW 1.12.1 install, or renders one shot of it to a PNG.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod args;
mod fly;
mod ground;
mod shot;
mod view;

use std::path::Path;

use bevy::prelude::*;

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
    let mut app = App::new();
    if let Err(e) = world::register_source(&mut app, Path::new(&data)) {
        eprintln!("cairn: {e}");
        return AppExit::error();
    }
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
            fly::FlyPlugin { pose: args.pose },
        )),
        args::Mode::Shot(out) => app.add_plugins((
            shot::headless_plugins(),
            shot::ShotPlugin {
                pose: args.pose,
                size: args.size,
                out,
            },
        )),
    };
    app.init_resource::<shot::ShotWaitsFor>()
        .add_plugins((world::LoadersPlugin, ground::GrassFieldPlugin))
        .run()
}
