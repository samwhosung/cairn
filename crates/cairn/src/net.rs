//! Playing with others through a server.

mod claims;
mod link;
mod others;
mod relay;
mod remote;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

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
use others::{BatchContext, Others};
#[cfg(test)]
pub use others::{Faults, OtherPlayer};
#[cfg(test)]
pub use remote::RemoteMotion;

const SPAWN_SPACING_YD: f32 = 2.5;
const SPAWNS: usize = 16;
const SEEN_EVERY: Duration = Duration::from_millis(500);

/// Present while the window plays with others; removed when its server goes.
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
    /// Returns at once; the welcome, or why the connection failed, arrives in later frames.
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

    /// Serves `map` in-process on 127.0.0.1:`port` and joins it; players stand beside `start` (WoW
    /// coordinates), facing `heading`.
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
        let mut net = Self::connect(SocketAddr::from(([127, 0, 0, 1], port)), hello);
        net.hosted = Some(hosted);
        Ok(net)
    }

    pub fn hosted_addr(&self) -> Option<SocketAddr> {
        self.hosted.as_ref().map(server::Running::addr)
    }

    #[cfg(test)]
    pub fn welcome(&self) -> Option<&Welcome> {
        self.welcomed.as_ref()
    }

    #[cfg(test)]
    pub fn corrections(&self) -> u32 {
        self.claims.as_ref().map_or(0, |c| c.corrections)
    }

    #[cfg(test)]
    pub fn claims_sent(&self) -> u32 {
        self.claims.as_ref().map_or(0, |c| c.sent)
    }

    #[cfg(test)]
    pub fn faults(&mut self) -> &mut Faults {
        &mut self.others.faults
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        if let Some(hosted) = self.hosted.take()
            && let Err(e) = hosted.stop()
        {
            warn!("the hosted server: {e}");
        }
    }
}

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
            (others::dress_remotes, others::fade_leaving).before(UnitSystems),
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
        let (frame, arrived) = match arrival {
            Arrival::Frame { bytes, arrived } => (bytes, arrived),
            Arrival::Gone { reason } => return alone(&mut commands, &mut net, reason),
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
                if let Some(addr) = net.hosted_addr() {
                    info!("hosting: others join with --connect {addr}");
                }
                net.claims = Some(Claims::new(&w.spawn));
                net.welcomed = Some(w);
            }
            Ok(ServerMessage::Batch(batch)) => {
                net.latest_tick = Some(batch.tick);
                let tick_ms = net.welcomed.map_or(0, |w| u32::from(w.tick_ms));
                let at = BatchContext {
                    server_ms: batch.tick.wrapping_mul(tick_ms),
                    own_pos: bevy_to_wow(player.pos),
                    arrived_real_ms: real_ms_at(&real, arrived),
                    now_real_ms: real.elapsed_secs_f64() * 1000.0,
                    game_secs: time.elapsed_secs(),
                };
                for record in batch {
                    match record {
                        Ok(Record::Correct { seq, why, movement }) => {
                            if let Some(claims) = &mut net.claims {
                                claims.correct(&mut player, seq, &movement);
                                warn!(
                                    "the server put the player back at {:?}, {} times now: {why}",
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
    let now = real.elapsed();
    if let Some(tick) = net.latest_tick
        && now.saturating_sub(net.seen_at) >= SEEN_EVERY
    {
        net.seen_at = now;
        net.link.send(&ClientMessage::Seen(tick));
    }
}

fn real_ms_at(real: &Time<Real>, instant: Instant) -> f64 {
    let now_ms = real.elapsed_secs_f64() * 1000.0;
    let Some(now) = real.last_update() else {
        return now_ms;
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    now_ms + ms(instant.saturating_duration_since(now)) - ms(now.saturating_duration_since(instant))
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

fn beside(start: [f32; 3], heading: f32) -> Vec<Spawn> {
    let right = [ops::sin(heading), -ops::cos(heading)];
    (0..SPAWNS)
        .map(|i| {
            let out = (i as f32 / 2.0).ceil() * SPAWN_SPACING_YD;
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
            (west_of_start(1) + SPAWN_SPACING_YD).abs() < 1e-4,
            "facing north, right is east"
        );
        assert!((west_of_start(2) - SPAWN_SPACING_YD).abs() < 1e-4);
        assert!((west_of_start(3) + 2.0 * SPAWN_SPACING_YD).abs() < 1e-4);
        assert!(
            spawns
                .iter()
                .all(|s| s.facing.abs() < 1e-6 && (s.pos[2] - 10.0).abs() < 1e-4)
        );
    }

    #[test]
    fn bytes_that_waited_for_the_frame_fall_on_the_clock_when_they_came() {
        let start = Instant::now();
        let mut real = Time::<Real>::new(start);
        real.update_with_instant(start);
        real.update_with_instant(start + Duration::from_secs(3));
        let at = |ms: u64| real_ms_at(&real, start + Duration::from_millis(ms));
        assert!((at(2500) - 2500.0).abs() < 1e-6, "{}", at(2500));
        assert!((at(3100) - 3100.0).abs() < 1e-6, "{}", at(3100));
    }
}
