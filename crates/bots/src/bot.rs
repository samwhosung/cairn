use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Pos, Record, ServerMessage, VERSION,
    Welcome, Wrapped,
};
use server::Spawn;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::check::{Checks, Limits, RELAYED_EPSILON_YD, Traffic, UnsentJudged};
use crate::ground::Ground;
use crate::mover::{Mover, Told};
use crate::region::Scenario;
use crate::track::{Track, plan};

const FRAME_MS: u64 = 50;
const JOIN_GRACE_MS: u32 = 2000;
const SWEEP_MS: u32 = 1000;
const FRAMES_PER_SEEN: u32 = 10;
const READ_BUF: usize = 64 << 10;
const HUMAN: u8 = 1;
const CLAIMS_KEPT: usize = 256;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn now_ms() -> u32 {
    ms_at(Instant::now())
}

fn ms_at(at: Instant) -> u32 {
    at.saturating_duration_since(*EPOCH).as_millis() as u32
}

#[derive(Default)]
struct Member {
    track: OnceLock<Track>,
    joined_ms: AtomicU32,
    gone: AtomicBool,
    claims: Mutex<VecDeque<Claimed>>,
}

#[derive(Clone, Copy)]
struct Claimed {
    pos: Pos,
    time: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub liars: usize,
    pub checkers: usize,
}

impl Roles {
    pub fn is_liar(&self, i: usize) -> bool {
        i < self.liars
    }

    pub fn is_checker(&self, i: usize) -> bool {
        !self.is_liar(i) && i < self.liars + self.checkers
    }
}

pub struct Crowd {
    pub scenario: Scenario,
    pub ground: Ground,
    by_id: Vec<Member>,
    pub roles: Roles,
    pub walks_end_ms: u32,
    pub limits: Limits,
    pub checks: Checks,
    pub traffic: Traffic,
    pub stop: AtomicBool,
}

impl Crowd {
    pub fn new(
        scenario: Scenario,
        ground: Ground,
        room: usize,
        roles: Roles,
        walks_end_ms: u32,
    ) -> Self {
        Self {
            scenario,
            ground,
            by_id: (0..room).map(|_| Member::default()).collect(),
            roles,
            walks_end_ms,
            limits: Limits::of_server(),
            checks: Checks::default(),
            traffic: Traffic::default(),
            stop: AtomicBool::new(false),
        }
    }

    fn track(&self, id: u32) -> Option<&Track> {
        self.by_id.get(id as usize)?.track.get()
    }

    fn claimed(&self, id: u32, m: &Movement) {
        let Some(member) = self.by_id.get(id as usize) else {
            return;
        };
        let mut claims = member.claims.lock().unwrap_or_else(PoisonError::into_inner);
        if claims.len() == CLAIMS_KEPT {
            claims.pop_front();
        }
        claims.push_back(Claimed {
            pos: Pos::of(m.pos),
            time: m.time,
        });
    }

    fn claimed_at(&self, id: u32, pos: Pos) -> Option<u32> {
        let claims = self
            .by_id
            .get(id as usize)?
            .claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        claims.iter().rev().find(|c| c.pos == pos).map(|c| c.time)
    }

    fn settled(&self, id: u32, now: u32) -> Option<&Track> {
        let m = self.by_id.get(id as usize)?;
        let joined = m.joined_ms.load(Ordering::Relaxed);
        let gone = m.gone.load(Ordering::Relaxed);
        (joined != 0 && !gone && now >= joined + JOIN_GRACE_MS)
            .then(|| m.track.get())
            .flatten()
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn ground(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[1]]
}

pub async fn run(i: usize, addr: SocketAddr, crowd: Arc<Crowd>) {
    let Some(stream) = connect(addr).await else {
        crowd.traffic.closed.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let _ = stream.set_nodelay(true);
    let (mut r, mut w) = stream.into_split();
    let liar = crowd.roles.is_liar(i);
    let mut hello = Vec::new();
    ClientMessage::Hello(Hello {
        version: VERSION,
        name: format!("{}{i}", if liar { "Liar" } else { "Bot" }),
        appearance: look(i),
    })
    .write(&mut hello);
    let mut frames = Frames::default();
    let welcome = match w.write_all(&hello).await {
        Ok(()) => welcomed(&mut r, &mut frames).await,
        Err(_) => None,
    };
    let Some(welcome) = welcome.filter(|w| (w.id as usize) < crowd.by_id.len()) else {
        crowd.traffic.closed.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let now = now_ms();
    let spawn = Spawn {
        pos: welcome.spawn.pos,
        facing: welcome.spawn.facing,
    };
    let track = plan(
        &crowd.scenario,
        &crowd.ground,
        &spawn,
        now + 100,
        crowd.walks_end_ms,
        u64::from(welcome.id) + 1,
        liar,
    );
    let id = welcome.id;
    crowd.claimed(id, &welcome.spawn);
    let member = &crowd.by_id[id as usize];
    let _ = member.track.set(track);
    member.joined_ms.store(now, Ordering::Relaxed);
    crowd.traffic.welcomed.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::unbounded_channel();
    let seen = Arc::new(AtomicU32::new(welcome.tick));
    let writer = tokio::spawn(write(id, spawn, w, crowd.clone(), rx, seen.clone()));
    let mut reader = Reader {
        me: id,
        welcome,
        welcomed_at: now,
        crowd: crowd.clone(),
        corrections: tx,
        seen,
        view: crowd.roles.is_checker(i).then(View::default),
        open: false,
        unsent: Unsent::default(),
        last_tick: None,
        next_sweep: now + JOIN_GRACE_MS,
        late_ms: Vec::new(),
    };
    reader.read(&mut r, frames).await;
    crowd.by_id[id as usize].gone.store(true, Ordering::Relaxed);
    crowd.traffic.closed.fetch_add(1, Ordering::Relaxed);
    writer.abort();
}

async fn connect(addr: SocketAddr) -> Option<TcpStream> {
    for attempt in 0..8u64 {
        if let Ok(s) = TcpStream::connect(addr).await {
            return Some(s);
        }
        tokio::time::sleep(Duration::from_millis(50 * (attempt + 1))).await;
    }
    None
}

fn look(i: usize) -> Appearance {
    let dial = |n: usize| (i / n % 5) as u8;
    Appearance {
        race: HUMAN,
        sex: (i % 2) as u8,
        skin: dial(2),
        face: dial(3),
        hair_style: dial(5),
        hair_color: dial(7),
        facial_hair: dial(11),
        equipment: [0; 10],
    }
}

async fn welcomed(r: &mut OwnedReadHalf, frames: &mut Frames) -> Option<Welcome> {
    let mut buf = vec![0u8; READ_BUF];
    loop {
        if let Some(frame) = frames.next_frame().ok()? {
            return match ServerMessage::read(frame) {
                Ok(ServerMessage::Welcome(w)) => Some(w),
                _ => None,
            };
        }
        let n = r.read(&mut buf).await.ok().filter(|&n| n > 0)?;
        frames.extend(&buf[..n]);
    }
}

async fn write(
    id: u32,
    spawn: Spawn,
    mut w: OwnedWriteHalf,
    crowd: Arc<Crowd>,
    mut corrections: mpsc::UnboundedReceiver<u32>,
    seen: Arc<AtomicU32>,
) {
    let Some(track) = crowd.track(id) else {
        return;
    };
    let mut mover = Mover::new(spawn.pos[2], spawn.facing);
    let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
    let (mut claims, mut bytes) = (Vec::<Movement>::new(), Vec::new());
    let mut frame = 0u32;
    while !crowd.stop.load(Ordering::Relaxed) {
        frame += 1;
        let frame_at = ms_at(ticker.tick().await.into_std());
        while let Ok(seq) = corrections.try_recv() {
            mover.correct(seq);
        }
        claims.clear();
        if mover.frame(frame_at, track, &crowd.ground, &mut claims) == Told::Lie {
            Checks::count(&crowd.checks.lying_frames);
        }
        bytes.clear();
        if frame.is_multiple_of(FRAMES_PER_SEEN) {
            ClientMessage::Seen(seen.load(Ordering::Relaxed)).write(&mut bytes);
        }
        if claims.is_empty() && bytes.is_empty() {
            continue;
        }
        for movement in &claims {
            crowd.claimed(id, movement);
            ClientMessage::Claim(Claim {
                ack: mover.ack,
                movement: *movement,
            })
            .write(&mut bytes);
        }
        if w.write_all(&bytes).await.is_err() {
            break;
        }
        let traffic = &crowd.traffic;
        traffic
            .bytes_out
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        traffic
            .claims
            .fetch_add(claims.len() as u64, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct View {
    by_slot: HashMap<u16, u32>,
    last_relayed: HashMap<u32, [f32; 3]>,
}

#[derive(Default)]
struct Unsent {
    decode_errors: u64,
    relayed_honest: UnsentJudged,
    relayed_liars: UnsentJudged,
    relayed_unclaimed: u64,
    stale_by_tier: [UnsentJudged; 3],
    unknown_moves: u64,
    double_appears: u64,
}

struct Reader {
    me: u32,
    welcome: Welcome,
    welcomed_at: u32,
    crowd: Arc<Crowd>,
    corrections: mpsc::UnboundedSender<u32>,
    seen: Arc<AtomicU32>,
    view: Option<View>,
    open: bool,
    unsent: Unsent,
    last_tick: Option<u32>,
    next_sweep: u32,
    late_ms: Vec<i64>,
}

impl Reader {
    async fn read(&mut self, r: &mut OwnedReadHalf, mut frames: Frames) {
        let mut buf = vec![0u8; READ_BUF];
        loop {
            loop {
                match frames.next_frame() {
                    Ok(Some(frame)) => self.frame(frame),
                    Ok(None) => break,
                    Err(_) => {
                        Checks::count(&self.crowd.traffic.decode_errors);
                        return;
                    }
                }
            }
            if self.crowd.stop.load(Ordering::Relaxed) {
                return;
            }
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let bytes = &self.crowd.traffic.bytes_in;
                    bytes.fetch_add(n as u64, Ordering::Relaxed);
                    frames.extend(&buf[..n]);
                }
            }
        }
    }

    fn frame(&mut self, frame: &[u8]) {
        let now = now_ms();
        let crowd = self.crowd.clone();
        let traffic = &crowd.traffic;
        let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(frame) else {
            Checks::count(&traffic.decode_errors);
            return;
        };
        Checks::count(&traffic.batches);
        if self.last_tick.is_some_and(|t| batch.tick != t + 1) {
            Checks::count(&traffic.gaps);
        }
        self.last_tick = Some(batch.tick);
        self.seen.store(batch.tick, Ordering::Relaxed);
        self.open = crowd.checks.is_open();
        let ticks = batch.tick.saturating_sub(self.welcome.tick);
        let due = self.welcomed_at + ticks * u32::from(self.welcome.tick_ms);
        if self.open {
            self.late_ms.push(i64::from(now) - i64::from(due));
        } else if let Some(&soonest) = self.late_ms.iter().min() {
            for &late in &self.late_ms {
                traffic.jitter.add((late - soonest) as u32);
            }
            self.late_ms.clear();
        }
        let here = self.view.as_ref().map_or([0.0; 3], |_| self.here(now));
        for record in batch {
            match record {
                Ok(Record::Appear {
                    slot, id, state, ..
                }) => self.appear(slot, id, state.pos, here),
                Ok(Record::Move { slot, pos, .. }) => self.moved(slot, Some(pos), now, here),
                Ok(Record::State { slot, state }) => self.moved(slot, Some(state.pos), now, here),
                Ok(Record::Turn { slot, .. }) => self.moved(slot, None, now, here),
                Ok(Record::Vanish { slot }) => self.vanish(slot),
                Ok(Record::Correct { seq, .. }) => self.corrected(seq),
                Err(_) => self.unsent.decode_errors += 1,
            }
        }
        self.hand_in();
        if self.view.is_some() && now >= self.next_sweep {
            self.next_sweep = now + SWEEP_MS;
            if self.open {
                self.sweep(now);
            }
        }
    }

    fn here(&self, now: u32) -> [f32; 3] {
        let spawn = self.welcome.spawn.pos;
        let Some(me) = self.crowd.track(self.me) else {
            return spawn;
        };
        let [x, y] = me.xy(now);
        [x, y, self.crowd.ground.height(x, y).unwrap_or(spawn[2])]
    }

    fn hand_in(&mut self) {
        let (checks, t) = (&self.crowd.checks, &mut self.unsent);
        Checks::absorb(&self.crowd.traffic.decode_errors, &mut t.decode_errors);
        checks.relayed_honest.absorb(&mut t.relayed_honest);
        checks.relayed_liars.absorb(&mut t.relayed_liars);
        Checks::absorb(&checks.relayed_unclaimed, &mut t.relayed_unclaimed);
        for (judged, unsent) in checks.stale_by_tier.iter().zip(&mut t.stale_by_tier) {
            judged.absorb(unsent);
        }
        Checks::absorb(&checks.unknown_moves, &mut t.unknown_moves);
        Checks::absorb(&checks.double_appears, &mut t.double_appears);
    }

    fn corrected(&self, seq: u32) {
        let checks = &self.crowd.checks;
        let liar = self.crowd.track(self.me).is_some_and(|t| t.lie.is_some());
        Checks::count(if liar {
            &checks.liar_corrections
        } else {
            &checks.honest_corrections
        });
        let _ = self.corrections.send(seq);
    }

    fn judge_relayed(&mut self, id: u32, relayed: Pos) {
        let crowd = &self.crowd;
        let Some(track) = crowd.track(id) else {
            return;
        };
        let Some(time) = crowd.claimed_at(id, relayed) else {
            self.unsent.relayed_unclaimed += 1;
            return;
        };
        let e = dist(ground(relayed.yards()), track.xy(time));
        let judged = if track.lie.is_some() {
            &mut self.unsent.relayed_liars
        } else {
            &mut self.unsent.relayed_honest
        };
        judged.judge(e, RELAYED_EPSILON_YD);
    }

    fn appear(&mut self, slot: u16, id: u32, pos: Wrapped, here: [f32; 3]) {
        let Some(view) = &mut self.view else { return };
        let relayed = pos.around(here);
        let doubled = view.by_slot.insert(slot, id).is_some()
            || view.last_relayed.insert(id, relayed.yards()).is_some();
        if self.open {
            self.unsent.double_appears += u64::from(doubled);
            self.judge_relayed(id, relayed);
        }
    }

    fn moved(&mut self, slot: u16, pos: Option<Wrapped>, now: u32, here: [f32; 3]) {
        let Some(view) = &mut self.view else { return };
        let Some(&id) = view.by_slot.get(&slot) else {
            self.unsent.unknown_moves += u64::from(self.open);
            return;
        };
        let relayed = pos.map(|p| p.around(here));
        let before = match relayed {
            Some(p) => view.last_relayed.insert(id, p.yards()),
            None => view.last_relayed.get(&id).copied(),
        };
        if !self.open {
            return;
        }
        let crowd = &self.crowd;
        if let (Some(before), Some(me), Some(them)) = (
            before,
            crowd.track(self.me),
            crowd.settled(id, now).filter(|t| t.lie.is_none()),
        ) {
            let truth = them.xy(now);
            let t = crowd.limits.tier(dist(me.xy(now), truth));
            let bound = crowd.limits.view_lag_bound_yd(t);
            self.unsent.stale_by_tier[t].judge(dist(truth, ground(before)), bound);
        }
        if let Some(p) = relayed {
            self.judge_relayed(id, p);
        }
    }

    fn vanish(&mut self, slot: u16) {
        let Some(view) = &mut self.view else { return };
        match view.by_slot.remove(&slot) {
            Some(id) => {
                view.last_relayed.remove(&id);
            }
            None => self.unsent.unknown_moves += u64::from(self.open),
        }
    }

    fn sweep(&self, now: u32) {
        let (Some(view), Some(me)) = (&self.view, self.crowd.track(self.me)) else {
            return;
        };
        if now < self.welcomed_at + JOIN_GRACE_MS {
            return;
        }
        let (checks, limits) = (&self.crowd.checks, &self.crowd.limits);
        let here = me.xy(now);
        for id in 0..self.crowd.by_id.len() as u32 {
            let Some(them) = self.crowd.settled(id, now) else {
                continue;
            };
            if id == self.me || them.lie.is_some() {
                continue;
            }
            let truth = them.xy(now);
            let d = dist(here, truth);
            match view.last_relayed.get(&id) {
                None if d <= limits.view_yd - limits.presence_slack_yd => {
                    Checks::count(&checks.missing);
                    checks.missing_depth_yd.note(limits.view_yd - d);
                }
                Some(&seen) => {
                    if d > limits.view_yd + limits.presence_slack_yd {
                        Checks::count(&checks.spurious);
                    }
                    let t = limits.tier(d);
                    let bound = limits.view_lag_bound_yd(t);
                    checks.swept_by_tier[t].judge(dist(truth, ground(seen)), bound);
                }
                None => {}
            }
        }
    }
}
