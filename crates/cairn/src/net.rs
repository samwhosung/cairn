//! Playing with others: the connection to a server, the welcome that places the player, the
//! player's claims and the server's corrections, the other players in view, and the window that
//! goes on alone when its server goes.

mod claims;
mod link;
mod relay;
mod remote;
mod units;

use std::net::SocketAddr;
use std::time::Duration;

use bevy::math::ops;
use bevy::prelude::*;
use bevy::time::Real;
use protocol::{Appearance, ClientMessage, Hello, Record, ServerMessage, VERSION, Welcome};
use server::Spawn;
use world::CurrentMap;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::{CharacterLook, UnitSystems};

use crate::args::Join;
use crate::player::{CameraRig, Player};
use claims::Claims;
use link::{Arrival, Link};
use units::{Others, Stamp};

/// Yards between the places a host sets its players, across its own heading.
const SPAWN_SPACING: f32 = 2.5;
const SPAWNS: usize = 16;
/// How often the window tells the server which tick it has taken in.
const SEEN_EVERY: Duration = Duration::from_millis(500);

/// The window's part in a shared world: its connection, and the server when the window hosts it.
#[derive(Resource)]
pub struct Net {
    link: Link,
    hosted: Option<server::Running>,
    welcomed: Option<Welcome>,
    claims: Option<Claims>,
    others: Others,
    latest_tick: Option<u32>,
    seen_at: Duration,
}

impl Net {
    /// Joins the server at `addr` as `hello` says.
    pub fn connect(addr: SocketAddr, hello: Hello) -> Self {
        Self {
            link: Link::open(addr, hello),
            hosted: None,
            welcomed: None,
            claims: None,
            others: Others::default(),
            latest_tick: None,
            seen_at: Duration::ZERO,
        }
    }

    /// Serves the world on `map` from this process, setting players beside `start` (WoW
    /// coordinates) along `heading`, and joins it.
    pub fn host(
        port: u16,
        map: u32,
        start: [f32; 3],
        heading: f32,
        hello: Hello,
    ) -> Result<Self, String> {
        let hosted = server::start(server::Config {
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            tick_threads: 1,
            io_threads: 1,
            map,
            spawns: beside(start, heading),
            ..server::Config::default()
        })
        .map_err(|e| format!("--host {port}: {e}"))?;
        Ok(Self {
            hosted: Some(hosted),
            ..Self::connect(SocketAddr::from(([127, 0, 0, 1], port)), hello)
        })
    }

    /// Where the hosted server listens.
    pub fn hosting(&self) -> Option<SocketAddr> {
        self.hosted.as_ref().map(server::Running::addr)
    }

    #[cfg(test)]
    pub fn welcome(&self) -> Option<&Welcome> {
        self.welcomed.as_ref()
    }

    /// How often the server has put the player back.
    #[cfg(test)]
    pub fn corrections(&self) -> u32 {
        self.claims.as_ref().map_or(0, |c| c.corrections)
    }

    /// Claims sent so far.
    #[cfg(test)]
    pub fn claims_sent(&self) -> u32 {
        self.claims.as_ref().map_or(0, |c| c.sent)
    }
}

/// The hello a window sends: its name and the look it walks in.
pub fn hello(name: String, look: &CharacterLook) -> Hello {
    Hello {
        version: VERSION,
        name,
        appearance: Appearance {
            race: look.race,
            sex: look.sex,
            skin: look.skin,
            face: look.face,
            hair_style: look.hair_style,
            hair_color: look.hair_color,
            facial_hair: look.facial_hair,
            equipment: look.equipment,
        },
    }
}

/// Joins as `join` says, starting from where the window would have started alone.
pub fn join(
    app: &mut App,
    join: Join,
    hello: Hello,
    map: u32,
    start: [f32; 3],
    heading: f32,
) -> Result<(), String> {
    let net = match join {
        Join::Connect(addr) => Net::connect(addr, hello),
        Join::Host(port) => Net::host(port, map, start, heading, hello)?,
    };
    app.insert_resource(net).add_plugins(NetPlugin);
    Ok(())
}

/// Takes in what the server sends and moves the other players before the frame's movement runs,
/// and claims the movement after it.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            (
                receive.run_if(resource_exists::<Net>),
                remote::drain_pending_moves,
                remote::extrapolate_remote_units,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (units::dress_remotes, units::fade_leaving).before(UnitSystems),
        )
        .add_systems(PostUpdate, claims::claim.run_if(resource_exists::<Net>));
    }
}

#[allow(clippy::too_many_arguments)]
fn receive(
    mut commands: Commands<'_, '_>,
    real: Res<'_, Time<Real>>,
    time: Res<'_, Time>,
    mut net: ResMut<'_, Net>,
    map: Res<'_, CurrentMap>,
    mut player: ResMut<'_, Player>,
    mut rigs: Query<'_, '_, &mut CameraRig>,
) {
    let alone = |commands: &mut Commands<'_, '_>, net: &mut Net, why: String| {
        warn!("{why}; playing on alone");
        net.others.leave_all(commands, time.elapsed_secs());
        commands.remove_resource::<Net>();
    };
    for arrival in net.link.arrivals() {
        let frame = match arrival {
            Arrival::Frame(frame) => frame,
            Arrival::Gone(reason) => return alone(&mut commands, &mut net, reason),
        };
        match ServerMessage::read(&frame) {
            Ok(ServerMessage::Welcome(w)) if w.map != map.id => {
                let why = format!(
                    "the server is on map {}, and this window walks map {}",
                    w.map, map.id
                );
                return alone(&mut commands, &mut net, why);
            }
            Ok(ServerMessage::Welcome(w)) => {
                place(&mut player, &w);
                for mut rig in &mut rigs {
                    rig.yaw = w.spawn.facing;
                }
                info!("joined as {} at {:?}", w.id, w.spawn.pos);
                if let Some(addr) = net.hosting() {
                    info!("hosting: others join with --connect {addr}");
                }
                net.claims = Some(Claims::new(&w.spawn));
                net.welcomed = Some(w);
            }
            Ok(ServerMessage::Batch(batch)) => {
                net.latest_tick = Some(batch.tick);
                let tick_ms = net.welcomed.map_or(0, |w| u32::from(w.tick_ms));
                let at = Stamp {
                    wire_ms: batch.tick.wrapping_mul(tick_ms),
                    me: bevy_to_wow(player.pos),
                    now_ms: real.elapsed_secs_f64() * 1000.0,
                    now_secs: time.elapsed_secs(),
                };
                for record in batch {
                    match record {
                        Ok(Record::Correct { seq, movement }) => {
                            if let Some(claims) = &mut net.claims {
                                claims.correct(&mut player, seq, &movement);
                                warn!(
                                    "the server put the player back at {:?}, {} times now",
                                    movement.pos, claims.corrections
                                );
                            }
                        }
                        Ok(record) => net.others.take(&mut commands, record, &at),
                        Err(e) => {
                            let why = format!("the server sent a broken batch ({e})");
                            return alone(&mut commands, &mut net, why);
                        }
                    }
                }
            }
            Err(e) => {
                let why = format!("the server sent what is not a message ({e})");
                return alone(&mut commands, &mut net, why);
            }
        }
    }
    let now = time.elapsed();
    if let Some(tick) = net.latest_tick
        && now.saturating_sub(net.seen_at) >= SEEN_EVERY
    {
        net.seen_at = now;
        net.link.send(&ClientMessage::Seen(tick));
    }
}

fn place(player: &mut Player, welcome: &Welcome) {
    player.pos = wow_to_bevy(welcome.spawn.pos);
    player.face_yaw = welcome.spawn.facing;
    player.model_yaw = welcome.spawn.facing;
    player.vel_y = 0.0;
    player.horiz_vel = Vec3::ZERO;
    player.airborne_since = None;
    player.settling = true;
}

/// The host's start, then places either side of it in turn, further out each pair.
fn beside(start: [f32; 3], heading: f32) -> Vec<Spawn> {
    let right = [ops::sin(heading), -ops::cos(heading)];
    (0..SPAWNS)
        .map(|i| {
            let out = (i as f32 / 2.0).ceil() * SPAWN_SPACING;
            let side = if i % 2 == 1 { out } else { -out };
            Spawn {
                pos: [
                    start[0] + right[0] * side,
                    start[1] + right[1] * side,
                    start[2],
                ],
                facing: heading,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_player_stands_to_the_hosts_right_and_the_third_to_its_left() {
        let spawns = beside([100.0, 50.0, 10.0], 0.0);
        let west_of_start = |i: usize| spawns[i].pos[1] - 50.0;
        assert_eq!(spawns.len(), SPAWNS);
        assert!(west_of_start(0).abs() < 1e-4 && (spawns[0].pos[0] - 100.0).abs() < 1e-4);
        assert!(
            (west_of_start(1) + SPAWN_SPACING).abs() < 1e-4,
            "facing north, right is east"
        );
        assert!((west_of_start(2) - SPAWN_SPACING).abs() < 1e-4);
        assert!((west_of_start(3) + 2.0 * SPAWN_SPACING).abs() < 1e-4);
        assert!(
            spawns
                .iter()
                .all(|s| s.facing.abs() < 1e-6 && (s.pos[2] - 10.0).abs() < 1e-4)
        );
    }
}
