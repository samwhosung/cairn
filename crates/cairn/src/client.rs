use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use world::unit::{BodySkin, CharacterLook};
use world::{CurrentMap, Install};

use crate::args::{self, Args, Mode};
use crate::view::Aim;
use crate::{fixture, net, note, player, shot, viewer};

pub fn assemble(
    app: &mut App,
    args: Args,
    install: &Install,
    map: CurrentMap,
    start: Aim,
    default_plugins: impl FnOnce(PluginGroupBuilder) -> PluginGroupBuilder,
) -> Result<(), net::CannotHost> {
    world::register_source(app, install);
    let aim = args.aim.unwrap_or(start);
    let pose = aim.pose();
    match args.mode {
        Mode::Window(joining) => {
            let look = character_look(args.look);
            let mut rng = world::rig::AnimRng::default();
            rng.seed_for_session(false);
            app.insert_resource(rng).add_plugins((
                default_plugins(DefaultPlugins.set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "cairn".into(),
                        resolution: args.size.into(),
                        ..Window::default()
                    }),
                    ..WindowPlugin::default()
                })),
                world::collision::CollisionPlugin,
                player::PlayerPlugin {
                    pose,
                    mode: if args.start_flying {
                        player::Mode::Fly
                    } else {
                        player::Mode::Walk
                    },
                    look: look.clone(),
                },
                sound_plugin(args.mute),
                note::NotePlugin {
                    dir: args.notes,
                    map: args.map,
                },
            ));
            let feet = pose.target.to_array();
            net::join(app, &joining, &look, map.id, feet, pose.heading)?;
        }
        Mode::Shot(out) => {
            app.add_plugins((
                default_plugins(shot::headless_plugins()),
                shot::ShotPlugin {
                    pose,
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
            app.add_plugins(world::collision::CollisionPlugin);
            if let Some(display) = args.display {
                app.add_plugins(fixture::FixturePlugin(display));
            }
        }
        Mode::View => {
            app.add_plugins((
                default_plugins(shot::headless_plugins()),
                viewer::ViewerPlugin {
                    aim,
                    size: args.size,
                    age: args.world_age,
                },
                world::collision::CollisionPlugin,
            ));
        }
    }
    app.insert_resource(map)
        .insert_resource(args.time)
        .insert_resource(world::FullScreenGlow(args.glow))
        .add_plugins((world::LoadersPlugin, world::WorldPlugin));
    Ok(())
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

pub fn character_look(look: args::Look) -> CharacterLook {
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

#[cfg(test)]
mod tests;
