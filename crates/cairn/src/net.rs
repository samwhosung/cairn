//! Playing through a server: the window's own, alone or hosting others, or another's.

mod claims;
mod link;
mod others;
mod relay;
mod remote;

use std::fmt;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bevy::math::ops;
use bevy::prelude::*;
use bevy::time::Real;
use protocol::{
    Appearance, Batch, ClientMessage, Hello, Record, ServerMessage, Show, VERSION, Welcome, Whose,
};
use server::Spawn;
use world::CurrentMap;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::{CharacterLook, UnitAttack, UnitShow, UnitSystems};

use crate::args::{Join, Joining};
use crate::player::{CameraRig, Player, PlayerBody};
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
const ACTION_BAR_KEYS: [KeyCode; 12] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::Digit0,
    KeyCode::Minus,
    KeyCode::Equal,
];

/// Present while the window plays through a server; removed when that server goes.
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
        Self::over(Link::open(addr, hello), None)
    }

    /// Serves `cfg` in-process and joins it as its host, with no socket between them.
    pub fn host(cfg: server::Config, hello: Hello) -> Result<Self, String> {
        let port = cfg.addr.map(|a| a.port());
        let hosted = server::start(cfg).map_err(|e| match port {
            Some(port) => format!("--host {port}: {e}"),
            None => format!("the window's own server did not start: {e}"),
        })?;
        Ok(Self::over(
            Link::in_process(hosted.connect_host(), hello),
            Some(hosted),
        ))
    }

    /// Joins through `server`, a connection from inside a server's process that the test drives.
    #[cfg(test)]
    pub fn in_process(server: server::InProcess, hello: Hello) -> Self {
        Self::over(Link::in_process(server, hello), None)
    }

    fn over(link: Link, hosted: Option<server::Running>) -> Self {
        Self {
            link,
            hosted,
            welcomed: None,
            claims: None,
            others: Others::default(),
            latest_tick: None,
            seen_at: Duration::ZERO,
        }
    }

    pub fn hosted_addr(&self) -> Option<SocketAddr> {
        self.hosted.as_ref().and_then(server::Running::addr)
    }

    #[cfg(test)]
    pub fn stop_hosted(&mut self) -> Option<std::io::Result<server::Summary>> {
        self.hosted.take().map(server::Running::stop)
    }

    #[cfg(test)]
    pub fn why_put_back(&self) -> Option<protocol::Why> {
        self.claims.as_ref().and_then(|c| c.why_put_back)
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
    pub fn teleports_sent(&self) -> u32 {
        self.claims.as_ref().map_or(0, |c| c.teleports)
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

/// The server a window runs for itself on `map`: players stand beside `start` (WoW coordinates),
/// facing `heading`; with a port, others join there.
pub fn own_server(port: Option<u16>, map: u32, start: [f32; 3], heading: f32) -> server::Config {
    server::Config {
        addr: port.map(|port| SocketAddr::from(([127, 0, 0, 1], port))),
        tick_threads: 1,
        io_threads: 1,
        map,
        spawns: beside(start, heading),
        ..server::Config::default()
    }
}

#[derive(Debug)]
pub struct CannotHost(String);

impl fmt::Display for CannotHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A window alone whose server fails to start walks on without one.
pub fn join(
    app: &mut App,
    joining: &Joining,
    look: &CharacterLook,
    map: u32,
    start: [f32; 3],
    heading: f32,
) -> Result<(), CannotHost> {
    let hello = hello(joining.name.clone(), look);
    let game = joining.game.as_ref().map(|g| {
        let (knobs, overlay) = (g.knobs.as_deref(), g.overlay.as_deref());
        catalog::from_files(&g.name, knobs, overlay, 0)
    });
    let game = game.transpose().map_err(CannotHost)?;
    let served = |port: Option<u16>| server::Config {
        game: game.clone(),
        world: joining.world.clone(),
        ..own_server(port, map, start, heading)
    };
    let net = match joining.how {
        Join::Connect(addr) => Net::connect(addr, hello),
        Join::Host(port) => Net::host(served(Some(port)), hello).map_err(CannotHost)?,
        Join::Alone => match Net::host(served(None), hello) {
            Ok(net) => net,
            Err(e) => {
                warn!("{e}; playing on alone");
                return Ok(());
            }
        },
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
        .add_systems(
            PostUpdate,
            (act, claims::claim).run_if(resource_exists::<Net>),
        );
    }
}

fn act(keys: Res<'_, ButtonInput<KeyCode>>, net: Res<'_, Net>) {
    if net.welcomed.is_none() {
        return;
    }
    for (number, key) in (1..).zip(ACTION_BAR_KEYS) {
        if keys.just_pressed(key) {
            net.link.send(&ClientMessage::Action(number));
        }
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
    mut own: Query<'_, '_, (Entity, &mut UnitShow), With<PlayerBody>>,
) {
    let alone =
        |commands: &mut Commands<'_, '_>, net: &mut Net, player: &mut Player, why: String| {
            warn!("{why}; playing on alone");
            player.rooted = false;
            net.others.leave_all(commands, time.elapsed_secs());
            commands.remove_resource::<Net>();
        };
    for arrival in net.link.arrivals() {
        let (frame, arrived) = match arrival {
            Arrival::Frame { bytes, arrived } => (bytes, arrived),
            Arrival::Gone { reason } => return alone(&mut commands, &mut net, &mut player, reason),
        };
        match ServerMessage::read(&frame) {
            Ok(ServerMessage::Welcome(w)) if w.map != map.id => {
                let why = format!(
                    "the server is on map {}, and this window walks map {}",
                    w.map, map.id
                );
                return alone(&mut commands, &mut net, &mut player, why);
            }
            Ok(ServerMessage::Welcome(w)) => {
                player.put(wow_to_bevy(w.spawn.pos), w.spawn.facing);
                player.settling = true;
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
                let stands = bevy_to_wow(player.pos);
                let mut at = BatchContext {
                    server_ms: batch.tick.wrapping_mul(tick_ms),
                    read_around: net
                        .claims
                        .as_ref()
                        .map_or(stands, |c| c.read_around(stands)),
                    arrived_real_ms: real_ms_at(&real, arrived),
                    now_real_ms: real.elapsed_secs_f64() * 1000.0,
                    game_secs: time.elapsed_secs(),
                };
                let Net { claims, others, .. } = &mut *net;
                let taken = take_batch(
                    batch,
                    &mut at,
                    claims.as_mut(),
                    others,
                    &mut player,
                    own.single_mut()
                        .ok()
                        .map(|(body, show)| (body, show.into_inner())),
                    &mut commands,
                );
                if let Err(e) = taken {
                    let why = format!("the server sent a broken batch ({e})");
                    return alone(&mut commands, &mut net, &mut player, why);
                }
            }
            Err(e) => {
                let why = format!("the server sent what is not a message ({e})");
                return alone(&mut commands, &mut net, &mut player, why);
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

fn take_batch(
    batch: Batch<'_>,
    at: &mut BatchContext,
    mut claims: Option<&mut Claims>,
    others: &mut Others,
    player: &mut Player,
    mut own: Option<(Entity, &mut UnitShow)>,
    commands: &mut Commands<'_, '_>,
) -> Result<(), protocol::Error> {
    let own_body = own.as_ref().map(|&(body, _)| body);
    for record in batch {
        match record? {
            #[cfg(test)]
            Record::Show { .. } if others.faults.no_show => {}
            Record::Show {
                whose,
                show: Show::Attack { target, outcome },
            } => {
                let body = |whose| match whose {
                    Whose::Own => own_body,
                    Whose::Slot(slot) => others.body_in(slot),
                };
                if let Some(attacker) = body(whose) {
                    commands.write_message(UnitAttack {
                        attacker,
                        target: target.and_then(body),
                        outcome: others::outcome_of(outcome),
                    });
                }
            }
            Record::Show {
                whose: Whose::Own,
                show,
            } => {
                if let Some((_, own)) = own.as_mut() {
                    others::apply_show(own, show);
                }
            }
            Record::Correct { seq, why, movement } => {
                if let Some(claims) = claims.as_deref_mut() {
                    claims.correct(player, seq, why, &movement);
                    at.read_around = movement.pos;
                    warn!(
                        "the server put the player back at {:?}, {} times now: {why}",
                        movement.pos, claims.corrections
                    );
                }
            }
            Record::Granted { movement } => {
                if let Some(claims) = claims.as_deref_mut() {
                    claims.granted(&movement);
                    at.read_around = movement.pos;
                }
            }
            Record::Place {
                seq,
                rooted,
                movement,
            } => {
                if let Some(claims) = claims.as_deref_mut() {
                    claims.place(player, seq, rooted, &movement);
                    at.read_around = movement.pos;
                    info!(
                        "the game put the player at {:?}, {} times now; rooted: {rooted}",
                        movement.pos, claims.placements
                    );
                }
            }
            record => others.take(commands, record, at),
        }
    }
    Ok(())
}

fn real_ms_at(real: &Time<Real>, instant: Instant) -> f64 {
    let now_ms = real.elapsed_secs_f64() * 1000.0;
    let Some(now) = real.last_update() else {
        return now_ms;
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    now_ms + ms(instant.saturating_duration_since(now)) - ms(now.saturating_duration_since(instant))
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
    use bevy::ecs::message::MessageCursor;
    use protocol::{
        Intro, LEN_BYTES, Movement, Outcome, Relay, Why, begin_batch, finish_frame, write_appear,
        write_correct, write_granted, write_move, write_place, write_show, write_vanish,
    };
    use world::unit::Outcome as Shown;

    use super::*;

    const STOOD: [f32; 3] = [-9000.0, 100.0, 50.0];
    const NEIGHBOUR: u16 = 3;

    fn at(x_east_of_where_it_stood: f32) -> Movement {
        Movement {
            pos: [STOOD[0] + x_east_of_where_it_stood, STOOD[1], STOOD[2]],
            ..Movement::default()
        }
    }

    struct Reader {
        app: App,
        claims: Claims,
        others: Others,
        player: Player,
        own: Option<(Entity, UnitShow)>,
    }

    impl Reader {
        fn new() -> Self {
            let mut app = App::new();
            app.init_resource::<Time>().add_message::<UnitAttack>();
            let mut player = Player::default();
            player.put(wow_to_bevy(STOOD), 0.0);
            Self {
                app,
                claims: Claims::new(&at(0.0)),
                others: Others::default(),
                player,
                own: None,
            }
        }

        fn take(&mut self, tick: u32, write: impl FnOnce(&mut Vec<u8>)) {
            let mut bytes = Vec::new();
            let start = begin_batch(&mut bytes, tick);
            write(&mut bytes);
            finish_frame(&mut bytes, start);
            let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&bytes[LEN_BYTES..]) else {
                panic!("a batch");
            };
            let real_ms = f64::from(tick) * 50.0;
            let mut at = BatchContext {
                server_ms: tick * 50,
                read_around: self.claims.read_around(bevy_to_wow(self.player.pos)),
                arrived_real_ms: real_ms,
                now_real_ms: real_ms,
                game_secs: 0.0,
            };
            let world = self.app.world_mut();
            take_batch(
                batch,
                &mut at,
                Some(&mut self.claims),
                &mut self.others,
                &mut self.player,
                self.own.as_mut().map(|(body, show)| (*body, show)),
                &mut world.commands(),
            )
            .expect("a whole batch");
            world.flush();
        }

        fn east_of_where_it_stood(&mut self, id: u32) -> Option<f32> {
            let world = self.app.world_mut();
            let mut remotes = world.query::<(&OtherPlayer, &RemoteMotion)>();
            remotes
                .iter(world)
                .find(|(p, _)| p.id == id)
                .map(|(_, m)| m.wow_pos[0] - STOOD[0])
        }

        fn appear(&mut self, tick: u32, slot: u16, id: u32, x_east: f32) {
            self.take(tick, |out| {
                let intro = Intro::new(id, "Neighbour", &Appearance::default());
                write_appear(out, slot, &intro, &Relay::of(&at(x_east)));
            });
        }
    }

    #[test]
    fn a_batch_is_read_after_its_correction_where_the_player_was_put_back() {
        let mut r = Reader::new();
        r.appear(20, NEIGHBOUR, 7, 10.0);
        r.player.put(wow_to_bevy(at(500.0).pos), 0.0);
        r.take(21, |out| {
            write_correct(out, 1, Why::Teleport, &at(0.0));
            write_move(out, NEIGHBOUR, &Relay::of(&at(11.0)));
        });
        let back = bevy_to_wow(r.player.pos);
        assert!((back[0] - STOOD[0]).abs() < 0.01, "put back to {back:?}");
        let east = r.east_of_where_it_stood(7).expect("in view");
        assert!(
            (east - 11.0).abs() < 0.01,
            "the neighbour read {east} yd east"
        );
    }

    #[test]
    fn a_landing_is_read_from_where_the_player_stood_until_the_server_takes_it() {
        let mut r = Reader::new();
        r.appear(20, NEIGHBOUR, 7, 9.0);
        let landing = Movement {
            time: 1000,
            ..at(500.0)
        };
        r.player.put(wow_to_bevy(landing.pos), 0.0);
        r.claims.teleported(&landing);
        r.take(21, |out| {
            write_move(out, NEIGHBOUR, &Relay::of(&at(10.0)));
        });
        let east = r.east_of_where_it_stood(7).expect("in view");
        assert!(
            (east - 10.0).abs() < 0.01,
            "the neighbour read {east} yd east"
        );
        r.take(22, |out| {
            write_granted(out, &landing);
            write_vanish(out, NEIGHBOUR);
            let intro = Intro::new(8, "Beside the landing", &Appearance::default());
            write_appear(out, NEIGHBOUR + 1, &intro, &Relay::of(&at(505.0)));
        });
        let east = r.east_of_where_it_stood(8).expect("in view");
        assert!(
            (east - 505.0).abs() < 0.01,
            "the new neighbour read {east} yd east"
        );
        r.take(23, |out| {
            write_move(out, NEIGHBOUR + 1, &Relay::of(&at(506.0)));
        });
        let east = r.east_of_where_it_stood(8).expect("in view");
        assert!(
            (east - 506.0).abs() < 0.01,
            "once taken, read {east} yd east"
        );
        assert_eq!(r.east_of_where_it_stood(7), None);
        let again = Movement {
            time: 2000,
            ..at(1000.0)
        };
        r.player.put(wow_to_bevy(again.pos), 0.0);
        r.claims.teleported(&again);
        r.take(24, |out| {
            write_move(out, NEIGHBOUR + 1, &Relay::of(&at(507.0)));
        });
        let east = r.east_of_where_it_stood(8).expect("in view");
        assert!(
            (east - 507.0).abs() < 0.01,
            "landing again, read {east} yd east"
        );
    }

    #[test]
    fn a_batch_is_read_after_a_placement_where_the_game_put_the_player() {
        let mut r = Reader::new();
        r.appear(20, NEIGHBOUR, 7, 9.0);
        let landing = Movement {
            time: 1000,
            ..at(100.0)
        };
        r.player.put(wow_to_bevy(landing.pos), 0.0);
        r.claims.teleported(&landing);
        r.take(21, |out| {
            write_place(out, 1, true, &at(400.0));
            write_vanish(out, NEIGHBOUR);
            let intro = Intro::new(8, "Beside the spawn", &Appearance::default());
            write_appear(out, NEIGHBOUR + 1, &intro, &Relay::of(&at(405.0)));
        });
        let put = bevy_to_wow(r.player.pos);
        assert!((put[0] - STOOD[0] - 400.0).abs() < 0.01, "put at {put:?}");
        assert!(r.player.rooted);
        let east = r.east_of_where_it_stood(8).expect("in view");
        assert!(
            (east - 405.0).abs() < 0.01,
            "the new neighbour read {east} yd east"
        );
        r.take(22, |out| {
            write_move(out, NEIGHBOUR + 1, &Relay::of(&at(406.0)));
        });
        let east = r.east_of_where_it_stood(8).expect("in view");
        assert!(
            (east - 406.0).abs() < 0.01,
            "with the landing given up for the placement, read {east} yd east"
        );
    }

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

    #[test]
    fn an_attack_is_read_as_the_bodies_it_names_and_one_by_a_stranger_not_at_all() {
        let mut r = Reader::new();
        let own = r.app.world_mut().spawn_empty().id();
        r.own = Some((own, UnitShow::default()));
        r.appear(20, NEIGHBOUR, 7, 10.0);
        let attack = |target, outcome| Show::Attack { target, outcome };
        r.take(21, |out| {
            let (neighbour, stranger) = (Whose::Slot(NEIGHBOUR), Whose::Slot(NEIGHBOUR + 1));
            write_show(out, neighbour, attack(Some(Whose::Own), Outcome::Crit));
            write_show(out, Whose::Own, attack(Some(neighbour), Outcome::Miss));
            write_show(out, neighbour, attack(Some(stranger), Outcome::Parry));
            write_show(out, stranger, attack(Some(Whose::Own), Outcome::Hit));
        });
        let neighbour = r.others.body_in(NEIGHBOUR).expect("the neighbour's body");
        let messages = r.app.world().resource::<Messages<UnitAttack>>();
        let read: Vec<UnitAttack> = MessageCursor::default().read(messages).copied().collect();
        let attack = |attacker, target, outcome| UnitAttack {
            attacker,
            target,
            outcome,
        };
        assert_eq!(
            read,
            [
                attack(neighbour, Some(own), Shown::Crit),
                attack(own, Some(neighbour), Shown::Miss),
                attack(neighbour, None, Shown::Parry)
            ]
        );
    }
}
