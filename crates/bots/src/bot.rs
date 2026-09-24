use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};

use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Record, ServerMessage, VERSION,
    Welcome,
};
use server::Spawn;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::check::{Checks, Limits, RELAYED_EPSILON_YD, Traffic};
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
        view: crowd.roles.is_checker(i).then(HashMap::new),
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

struct Seen {
    pos: [f32; 3],
}

struct Reader {
    me: u32,
    welcome: Welcome,
    welcomed_at: u32,
    crowd: Arc<Crowd>,
    corrections: mpsc::UnboundedSender<u32>,
    seen: Arc<AtomicU32>,
    view: Option<HashMap<u32, Seen>>,
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
        let ticks = batch.tick.saturating_sub(self.welcome.tick);
        let due = self.welcomed_at + ticks * u32::from(self.welcome.tick_ms);
        if crowd.checks.is_open() {
            self.late_ms.push(i64::from(now) - i64::from(due));
        } else if let Some(&soonest) = self.late_ms.iter().min() {
            for &late in &self.late_ms {
                traffic.jitter.add((late - soonest) as u32);
            }
            self.late_ms.clear();
        }
        for record in batch {
            Checks::count(&traffic.records);
            match record {
                Ok(Record::Appear { id, movement, .. }) => self.appear(id, &movement),
                Ok(Record::Move { id, movement }) => self.moved(id, &movement, now),
                Ok(Record::Vanish { id }) => {
                    if let Some(view) = &mut self.view {
                        view.remove(&id);
                    }
                }
                Ok(Record::Correct { seq, .. }) => self.corrected(seq),
                Err(_) => Checks::count(&traffic.decode_errors),
            }
        }
        if self.view.is_some() && now >= self.next_sweep {
            self.next_sweep = now + SWEEP_MS;
            if crowd.checks.is_open() {
                self.sweep(now);
            }
        }
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

    fn judge_relayed(&self, id: u32, movement: &Movement) {
        if let Some(track) = self.crowd.track(id)
            && self.crowd.checks.is_open()
        {
            let e = dist(ground(movement.pos), track.xy(movement.time));
            let checks = &self.crowd.checks;
            let judged = if track.lie.is_some() {
                &checks.relayed_liars
            } else {
                &checks.relayed_honest
            };
            judged.judge(e, RELAYED_EPSILON_YD);
        }
    }

    fn appear(&mut self, id: u32, movement: &Movement) {
        let Some(view) = &self.view else { return };
        if view.contains_key(&id) && self.crowd.checks.is_open() {
            Checks::count(&self.crowd.checks.double_appears);
        }
        self.judge_relayed(id, movement);
        if let Some(view) = &mut self.view {
            view.insert(id, Seen { pos: movement.pos });
        }
    }

    fn moved(&mut self, id: u32, movement: &Movement, now: u32) {
        let Some(view) = &self.view else { return };
        let crowd = &self.crowd;
        let checks = &crowd.checks;
        match view.get(&id) {
            None if checks.is_open() => Checks::count(&checks.unknown_moves),
            Some(prev) if checks.is_open() => {
                if let (Some(me), Some(them)) = (
                    crowd.track(self.me),
                    crowd.settled(id, now).filter(|t| t.lie.is_none()),
                ) {
                    let truth = them.xy(now);
                    let t = crowd.limits.tier(dist(me.xy(now), truth));
                    let bound = crowd.limits.view_lag_bound_yd(t);
                    checks.stale_by_tier[t].judge(dist(truth, ground(prev.pos)), bound);
                }
            }
            _ => {}
        }
        self.judge_relayed(id, movement);
        if let Some(view) = &mut self.view {
            view.insert(id, Seen { pos: movement.pos });
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
            match view.get(&id) {
                None if d <= limits.view_yd - limits.presence_slack_yd => {
                    Checks::count(&checks.missing);
                    checks.missing_depth_yd.note(limits.view_yd - d);
                }
                Some(seen) => {
                    if d > limits.view_yd + limits.presence_slack_yd {
                        Checks::count(&checks.spurious);
                    }
                    let t = limits.tier(d);
                    let bound = limits.view_lag_bound_yd(t);
                    checks.swept_by_tier[t].judge(dist(truth, ground(seen.pos)), bound);
                }
                None => {}
            }
        }
    }
}
