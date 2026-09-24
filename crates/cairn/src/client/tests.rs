use std::sync::Arc;

use bevy::app::PluginGroupBuilder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use bevy::render::ExtractSchedule;
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy::winit::WinitPlugin;
use world::unit::UnitSystems;
use world::{CurrentMap, Install, WorldSystems};

use super::assemble;
use crate::args;

const FIRST_FRAMES: usize = 3;

/// winit's event loop needs the main thread, which a test does not run on, and pipelined rendering
/// moves the render app onto a thread of its own on cleanup, out of the check's reach.
fn without_a_window(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    plugins
        .disable::<WinitPlugin>()
        .disable::<PipelinedRenderingPlugin>()
}

fn client(argv: &str) -> App {
    let args = args::parse(argv.split_whitespace().map(str::to_owned)).expect("the arguments");
    let install = Install(Arc::new(mpq::Chain::default()));
    let map = CurrentMap {
        id: 0,
        directory: "Azeroth".into(),
    };
    let mut app = App::new();
    assemble(&mut app, args, &install, map, without_a_window);
    app
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
