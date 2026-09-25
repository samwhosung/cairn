use std::path::PathBuf;
use std::time::Instant;

use game::Delivery;
use protocol::{Appearance, ClientMessage, Hello, Pos, VERSION};
use rayon::prelude::*;
use server::{Config, Input, InputOrder, Refusal, Saving, Spawn, Stepper, TickStats, Why};

use super::client::{Accepted, Brief, Client, Delivered, Seen, Taken, Tally, World};
use super::played::Played;
use super::spec::{Group, Script, Spec, WorldFile};
use crate::ground::Ground;
use crate::lie::{Lie, same_bits};
use crate::region;
use crate::track::Pace;

const ROUTE_SLACK_MS: u32 = 10_000;

#[derive(Clone, Debug, Default)]
pub struct Account {
    awaited_ack: u32,
    pub claims: u64,
    pub accepted: u64,
    pub stale: u64,
    pub refused: [u64; Why::ALL.len()],
    pub lies_refused: u64,
    pub lies_stale: u64,
    pub lies_accepted: u64,
    pub first_caught_ms: Option<u32>,
    pub first_through_ms: Option<u32>,
    /// The furthest an accepted lie stood from the body that told it.
    pub past_honest_yd: f32,
}

pub struct Outcome {
    pub groups: Vec<usize>,
    pub accounts: Vec<Account>,
    pub tallies: Vec<Tally>,
    pub ticks: Vec<TickStats>,
    pub tick_ms: u16,
    pub wall_s: f64,
    pub game: Option<Played>,
}

impl Outcome {
    pub fn zeroed(spec: &Spec) -> Self {
        let groups: Vec<usize> = spec
            .groups
            .iter()
            .enumerate()
            .flat_map(|(i, g)| std::iter::repeat_n(i, g.count))
            .collect();
        let liars = liar_of(spec, &groups).iter().flatten().count();
        Self {
            accounts: vec![Account::default(); groups.len()],
            tallies: vec![
                Tally {
                    seen: vec![Seen::default(); liars],
                    ..Tally::default()
                };
                groups.len()
            ],
            groups,
            ticks: Vec::new(),
            tick_ms: spec.tick_ms,
            wall_s: 0.0,
            game: spec.game.as_ref().map(Played::zeroed),
        }
    }
}

/// For each bot, which liar it is, counting liars in order; `None` for an honest bot.
pub fn liar_of(spec: &Spec, groups: &[usize]) -> Vec<Option<usize>> {
    let mut next = 0;
    groups
        .iter()
        .map(|&g| {
            spec.groups[g].lie.map(|_| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct Orders {
    pub inputs: InputOrder,
    pub letters: Delivery,
}

pub fn run(
    spec: &Spec,
    ground: &Ground,
    threads: usize,
    orders: Orders,
) -> Result<Outcome, String> {
    let briefs = roster(spec, ground)?;
    let groups: Vec<usize> = briefs.iter().map(|b| b.group).collect();
    let liar_of = liar_of(spec, &groups);
    let liars = liar_of.iter().flatten().count();
    let file = WorldAt::of(spec);
    let cfg = config(spec, &briefs, threads, &file);
    let mut stepper =
        Stepper::new(&cfg, orders.inputs, orders.letters).map_err(|e| format!("a server: {e}"))?;
    let mut played = spec.game.as_ref().map(Played::zeroed);
    stepper.keep_refusals();
    let mut clients = connect(spec, briefs, &stepper, liars);
    let mut accounts = vec![Account::default(); clients.len()];
    let mut accepted = vec![Taken::default(); liars];
    let tick_ms = u32::from(cfg.tick_ms);
    let ticks = (spec.seconds * 1000.0 / tick_ms as f32).ceil() as u32;
    let mut stats = Vec::with_capacity(ticks as usize);
    let (mut inputs, mut delivered) = (Vec::new(), Vec::new());
    let began = Instant::now();
    for k in 0..ticks {
        let now = (k + 1) * tick_ms;
        let world = World {
            place: &spec.place,
            ground,
            liar_of: &liar_of,
            accepted: &accepted,
            view_radius: spec.view.radius,
        };
        stepper
            .pool()
            .install(|| clients.par_iter_mut().for_each(|c| c.advance(now, &world)));
        inputs.clear();
        delivered.clear();
        let next = stepper.next_tick();
        for c in &mut clients {
            c.deliver(now, next, &mut inputs, &mut delivered);
        }
        for s in &inputs {
            if let (Input::Join(_), Some(l)) = (&s.input, liar_of[s.conn as usize]) {
                accepted[l].push(Accepted {
                    pos: Pos::of(clients[s.conn as usize].brief.spawn.pos),
                    tick: next,
                    off_body_yd: 0.0,
                });
            }
        }
        let stepped = stepper.tick(&inputs);
        stats.push(stepped);
        let refusals = stepper.take_refusals();
        settle(
            &mut accounts,
            &delivered,
            &refusals,
            next,
            now,
            &liar_of,
            &mut accepted,
        )?;
        for id in stepper.placed_last_tick() {
            let acc = &mut accounts[id as usize];
            acc.awaited_ack = acc.awaited_ack.wrapping_add(1);
        }
        stepper
            .pool()
            .install(|| clients.par_iter_mut().for_each(|c| c.receive(now)));
        if let Some(p) = &mut played {
            p.check(next, stepped.hash, &stepper, &clients);
        }
    }
    if let Some(p) = &mut played {
        p.finish(&stepper, &clients);
    }
    stepper
        .finish()
        .map_err(|e| format!("the world's file: {e}"))?;
    let wall_s = began.elapsed().as_secs_f64();
    for c in &clients {
        if c.tally.welcomed_as != Some(c.conn) {
            let what = c
                .tally
                .welcomed_as
                .map_or("never welcomed".into(), |id| format!("welcomed as {id}"));
            return Err(format!("bot {} was {what}; bots join in order", c.conn));
        }
    }
    Ok(Outcome {
        groups,
        accounts,
        tallies: clients.into_iter().map(|c| c.tally).collect(),
        ticks: stats,
        tick_ms: cfg.tick_ms,
        wall_s,
        game: played,
    })
}

fn config(spec: &Spec, briefs: &[Brief], threads: usize, file: &WorldAt) -> Config {
    Config {
        tick_threads: threads,
        spawns: briefs.iter().map(|b| b.spawn).collect(),
        limits: spec.limits,
        view: spec.view,
        tick_ms: spec.tick_ms,
        game: spec.game.clone(),
        world: file.path.clone(),
        saving: spec.drops.map_or(Saving::Held, Saving::Drops),
        ..Config::default()
    }
}

/// The file a run keeps its world in; a fresh one is removed once the run is over.
struct WorldAt {
    path: Option<PathBuf>,
    fresh: bool,
}

impl WorldAt {
    fn of(spec: &Spec) -> Self {
        match &spec.world {
            WorldFile::Fresh => {
                let name: String = spec
                    .name
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_nanos());
                let file = format!("cairn-{name}-{}-{nanos}.sqlite", std::process::id());
                Self {
                    path: Some(std::env::temp_dir().join(file)),
                    fresh: true,
                }
            }
            WorldFile::At(path) => Self {
                path: Some(path.clone()),
                fresh: false,
            },
            WorldFile::Unsaved => Self {
                path: None,
                fresh: false,
            },
        }
    }
}

impl Drop for WorldAt {
    fn drop(&mut self) {
        let Some(path) = self.path.as_ref().filter(|_| self.fresh) else {
            return;
        };
        for end in ["", "-wal", "-shm", "-lock"] {
            let mut name = path.as_os_str().to_owned();
            name.push(end);
            let _ = std::fs::remove_file(name);
        }
    }
}

fn connect(spec: &Spec, briefs: Vec<Brief>, stepper: &Stepper, liars: usize) -> Vec<Client> {
    let lag = (spec.delay_ms, spec.jitter_ms);
    (0..)
        .zip(briefs)
        .map(|(conn, brief)| {
            let hello = hello(&spec.groups[brief.group].name, conn as usize);
            Client::new(conn, brief, stepper.connect(conn), lag, hello, liars)
        })
        .collect()
}

fn hello(group: &str, i: usize) -> Vec<u8> {
    let mut frame = Vec::new();
    ClientMessage::Hello(Hello {
        version: VERSION,
        name: format!("{group}{i}"),
        appearance: Appearance {
            race: 1,
            sex: (i % 2) as u8,
            ..Appearance::default()
        },
    })
    .write(&mut frame);
    frame
}

fn settle(
    accounts: &mut [Account],
    delivered: &[(u32, Delivered)],
    refusals: &[Refusal],
    tick: u32,
    now: u32,
    liar_of: &[Option<usize>],
    accepted: &mut [Taken],
) -> Result<(), String> {
    debug_assert!(delivered.is_sorted_by_key(|(conn, _)| *conn));
    let mut why: Vec<Option<Why>> = vec![None; delivered.len()];
    for r in refusals {
        let from = delivered.partition_point(|(conn, _)| *conn < r.id);
        let hit = (from..delivered.len())
            .take_while(|&i| delivered[i].0 == r.id)
            .find(|&i| why[i].is_none() && same_bits(&r.claim, &delivered[i].1.claim.movement));
        let Some(i) = hit else {
            return Err(format!(
                "tick {}: the server refused a claim of bot {} no bot sent: {:?}",
                r.tick, r.id, r.claim
            ));
        };
        why[i] = Some(r.why);
    }
    for ((conn, d), refused) in delivered.iter().zip(why) {
        let acc = &mut accounts[*conn as usize];
        acc.claims += 1;
        let lied = d.made_in.lied;
        if let Some(why) = refused {
            acc.awaited_ack = acc.awaited_ack.wrapping_add(1);
            acc.refused[why as usize] += 1;
            if lied {
                acc.lies_refused += 1;
                acc.first_caught_ms.get_or_insert(now);
            }
        } else if d.claim.ack != acc.awaited_ack {
            acc.stale += 1;
            acc.lies_stale += u64::from(lied);
        } else {
            acc.accepted += 1;
            let off_body_yd = distance(d.claim.movement.pos, d.made_in.truth.pos);
            if lied {
                acc.lies_accepted += 1;
                acc.first_through_ms.get_or_insert(now);
                acc.past_honest_yd = acc.past_honest_yd.max(off_body_yd);
            }
            if let Some(liar) = liar_of[*conn as usize] {
                accepted[liar].push(Accepted {
                    pos: Pos::of(d.claim.movement.pos),
                    tick,
                    off_body_yd,
                });
            }
        }
    }
    Ok(())
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn roster(spec: &Spec, ground: &Ground) -> Result<Vec<Brief>, String> {
    let sampled = spec
        .groups
        .iter()
        .filter(|g| g.spawn.is_none() && g.control_of.is_none())
        .map(|g| g.count)
        .sum();
    let mut sampled = region::spawns(&spec.place, ground, sampled, spec.seed)?.into_iter();
    let run_ms = (spec.seconds * 1000.0) as u32;
    let mut first_of = Vec::with_capacity(spec.groups.len());
    let mut briefs: Vec<Brief> = Vec::new();
    for (gi, g) in spec.groups.iter().enumerate() {
        first_of.push(briefs.len());
        for i in 0..g.count {
            let (spawn, seed) = if let Some(liar) = g.control_of {
                let b = &briefs[first_of[liar] + i];
                (b.spawn, b.seed)
            } else {
                let spawn = match g.spawn {
                    Some(xy) => placed(g, xy, ground)?,
                    None => sampled.next().ok_or("too few spawns")?,
                };
                (spawn, spec.seed ^ ((gi as u64) << 32) ^ i as u64)
            };
            let spawn = match g.script {
                Script::Wander | Script::Fight => spawn,
                Script::Line | Script::Stand => Spawn {
                    facing: g.facing_deg.to_radians(),
                    ..spawn
                },
            };
            briefs.push(brief(g, gi, spawn, seed, run_ms));
        }
    }
    Ok(briefs)
}

fn placed(g: &Group, [x, y]: [f32; 2], ground: &Ground) -> Result<Spawn, String> {
    let z = if g.surface {
        ground.surface(x, y)
    } else {
        ground.height(x, y)
    };
    let z = z.ok_or(format!("bots.{}.spawn {x} {y} is off the ground", g.name))?;
    Ok(Spawn {
        pos: [x, y, z],
        facing: g.facing_deg.to_radians(),
    })
}

fn brief(g: &Group, group: usize, spawn: Spawn, seed: u64, run_ms: u32) -> Brief {
    let stretch = g.lie.as_ref().map_or(1.0, Lie::reach);
    let route_until_ms = g.clock_at_start_ms + (run_ms as f32 * stretch) as u32 + ROUTE_SLACK_MS;
    Brief {
        group,
        spawn,
        script: g.script,
        pace: Pace {
            speed: g.speed,
            gait: g.gait,
            stop_yd: g.stop_yd,
            jump_every_ms: g.jump_every_s.map(|s| (s * 1000.0) as u32),
            surface: g.surface,
        },
        lies: g.lie.into_iter().collect(),
        frame_hz: g.frame_hz,
        claims: g.claims,
        clock_at_start_ms: g.clock_at_start_ms,
        seed,
        route_until_ms,
        swing_action: g.swing_action,
        heeds_roots: g.heeds_roots,
        drops: g.drops,
    }
}
