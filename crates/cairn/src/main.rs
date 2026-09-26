//! The cairn client: walks a window through a WoW 1.12.1 install, renders one shot of it to a PNG, or draws a zone of it from above.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod args;
mod atlas;
mod client;
mod fixture;
mod fly;
mod net;
mod note;
mod player;
mod shot;
mod view;

use std::path::Path;

use bevy::prelude::*;
use world::unit::CharacterTables;
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
    if argv.first().is_some_and(|arg| arg == "atlas") {
        return atlas::main(&argv[1..]);
    }
    let mut args = match args::parse(argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("cairn: {e}\n\n{}", args::USAGE);
            return AppExit::from_code(2);
        }
    };
    if let args::Mode::Window(joining) = &mut args.mode
        && matches!(joining.how, args::Join::Host(_))
        && joining.world.is_none()
    {
        joining.world = server::default_world(joining.game.as_ref().map(|g| g.name.as_str()));
    }
    if matches!(args.mode, args::Mode::Window(_)) && args.notes.is_none() {
        args.notes = server::data_dir().map(|dir| dir.join("notes"));
    }
    args.patch = args
        .patch
        .map(|patch| std::path::absolute(&patch).unwrap_or(patch));
    let install = match install(args.patch.as_deref()) {
        Ok(install) => install,
        Err(exit) => return exit,
    };
    let map = match CurrentMap::find(&install.0, &args.map) {
        Ok(map) => map,
        Err(e) => {
            eprintln!("cairn: {e}");
            return AppExit::from_code(2);
        }
    };
    let mut app = App::new();
    if matches!(args.mode, args::Mode::Window(_)) {
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
    if let Err(e) = client::assemble(&mut app, args, &install, map, std::convert::identity) {
        eprintln!("cairn: {e}");
        return AppExit::from_code(2);
    }
    app.run()
}

fn install(patch: Option<&Path>) -> Result<Install, AppExit> {
    let Some(data) = std::env::var_os("WOW_DATA") else {
        eprintln!("cairn: set WOW_DATA to the Data directory of a WoW 1.12.1 install");
        return Err(AppExit::from_code(2));
    };
    let data = Path::new(&data);
    match patch {
        Some(patch) => Install::open_patched(data, patch),
        None => Install::open(data),
    }
    .map_err(|e| {
        eprintln!("cairn: {e}");
        AppExit::error()
    })
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
