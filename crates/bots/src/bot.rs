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

use crate::check::{Checks, EXACT_YD, PRESENCE_SLACK_YD, Traffic, VIEW_YD, bound_yd, tier};
use crate::ground::Ground;
use crate::mover::Mover;
use crate::region::Scenario;
use crate::track::{Track, plan};

/// How often a bot moves and reports, ms.
const FRAME_MS: u64 = 50;
/// How long after a bot joins before others must see it and before it checks what it sees.
const SETTLE_MS: u32 = 2000;
const SWEEP_MS: u32 = 1000;
const READ_BUF: usize = 64 << 10;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Milliseconds on the clock every bot shares, which is also every bot's client clock.
pub fn now_ms() -> u32 {
    EPOCH.elapsed().as_millis() as u32
}

/// Every bot's walk by entity id, and what the bots count between them.
pub struct Crowd {
    pub scenario: Scenario,
    pub ground: Ground,
    tracks: Vec<OnceLock<Track>>,
    joined: Vec<AtomicU32>,
    gone: Vec<AtomicBool>,
    /// Bots below this index lie; the next `checkers` check what they see.
    pub liars: usize,
    pub checkers: usize,
    /// When the walks end, ms.
    pub until: u32,
    pub checks: Checks,
    pub traffic: Traffic,
    pub stop: AtomicBool,
}

impl Crowd {
    pub fn new(
        scenario: Scenario,
        ground: Ground,
        room: usize,
        roles: (usize, usize),
        until: u32,
    ) -> Self {
        Self {
            scenario,
            ground,
            tracks: (0..room).map(|_| OnceLock::new()).collect(),
            joined: (0..room).map(|_| AtomicU32::new(0)).collect(),
            gone: (0..room).map(|_| AtomicBool::new(false)).collect(),
            liars: roles.0,
            checkers: roles.1,
            until,
            checks: Checks::default(),
            traffic: Traffic::default(),
            stop: AtomicBool::new(false),
        }
    }

    fn track(&self, id: u32) -> Option<&Track> {
        self.tracks.get(id as usize)?.get()
    }

    /// The walk of a bot that is in the world and has been long enough to be seen.
    fn settled(&self, id: u32, now: u32) -> Option<&Track> {
        let joined = self.joined.get(id as usize)?.load(Ordering::Relaxed);
        let gone = self.gone.get(id as usize)?.load(Ordering::Relaxed);
        (joined != 0 && !gone && now >= joined + SETTLE_MS)
            .then(|| self.track(id))
            .flatten()
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn ground(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[1]]
}

/// Runs bot `i` against `addr` until the crowd stops or the server closes the connection.
pub async fn run(i: usize, addr: SocketAddr, crowd: Arc<Crowd>) {
    let Some(stream) = connect(addr).await else {
        crowd.traffic.closed.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let _ = stream.set_nodelay(true);
    let (mut r, mut w) = stream.into_split();
    let mut hello = Vec::new();
    ClientMessage::Hello(Hello {
        version: VERSION,
        name: format!("Bot{i}"),
        appearance: look(i),
    })
    .write(&mut hello);
    let mut frames = Frames::default();
    let welcome = match w.write_all(&hello).await {
        Ok(()) => welcomed(&mut r, &mut frames).await,
        Err(_) => None,
    };
    let Some(welcome) = welcome.filter(|w| (w.id as usize) < crowd.tracks.len()) else {
        crowd.traffic.closed.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let now = now_ms();
    let spawn = Spawn {
        pos: welcome.spawn.pos,
        facing: welcome.spawn.facing,
    };
    let liar = i < crowd.liars;
    let track = plan(
        &crowd.scenario,
        &crowd.ground,
        &spawn,
        (now + 100, crowd.until),
        u64::from(welcome.id) + 1,
        liar,
    );
    let id = welcome.id;
    let _ = crowd.tracks[id as usize].set(track);
    crowd.joined[id as usize].store(now, Ordering::Relaxed);
    crowd.traffic.welcomed.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::unbounded_channel();
    let writer = tokio::spawn(write(id, spawn, w, crowd.clone(), rx));
    let checker = !liar && i < crowd.liars + crowd.checkers;
    let mut reader = Reader {
        me: id,
        welcome,
        welcomed_at: now,
        crowd: crowd.clone(),
        corrections: tx,
        view: checker.then(HashMap::new),
        last_tick: None,
        next_sweep: now + SETTLE_MS,
        late: Vec::new(),
    };
    reader.read(&mut r, frames).await;
    crowd.gone[id as usize].store(true, Ordering::Relaxed);
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

/// A human of either sex with customization choices picked by `i`.
fn look(i: usize) -> Appearance {
    let dial = |n: usize| (i / n % 5) as u8;
    Appearance {
        race: 1,
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
) {
    let Some(track) = crowd.track(id) else {
        return;
    };
    let mut mover = Mover::new(spawn.pos[2], spawn.facing);
    let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut claims, mut bytes) = (Vec::<Movement>::new(), Vec::new());
    while !crowd.stop.load(Ordering::Relaxed) {
        ticker.tick().await;
        while let Ok(seq) = corrections.try_recv() {
            mover.correct(seq);
        }
        claims.clear();
        if mover.frame(now_ms(), track, &crowd.ground, &mut claims) {
            Checks::count(&crowd.checks.lies);
        }
        if claims.is_empty() {
            continue;
        }
        bytes.clear();
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

/// Where a checking bot last saw another.
struct Seen {
    pos: [f32; 3],
}

struct Reader {
    me: u32,
    welcome: Welcome,
    welcomed_at: u32,
    crowd: Arc<Crowd>,
    corrections: mpsc::UnboundedSender<u32>,
    view: Option<HashMap<u32, Seen>>,
    last_tick: Option<u32>,
    next_sweep: u32,
    /// How late each batch came against the tick clock while the window was open, ms.
    late: Vec<i64>,
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
        let ticks = batch.tick.saturating_sub(self.welcome.tick);
        let due = self.welcomed_at + ticks * u32::from(self.welcome.tick_ms);
        if crowd.checks.is_open() {
            self.late.push(i64::from(now) - i64::from(due));
        } else if let Some(&soonest) = self.late.iter().min() {
            for &late in &self.late {
                traffic.lag((late - soonest) as u32);
            }
            self.late.clear();
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

    /// Judges a relayed position against where its bot was when it claimed it.
    fn exact(&self, id: u32, movement: &Movement) {
        if let Some(track) = self.crowd.track(id)
            && self.crowd.checks.is_open()
        {
            let e = dist(ground(movement.pos), track.xy(movement.time));
            self.crowd.checks.exact.judge(e, EXACT_YD);
        }
    }

    fn appear(&mut self, id: u32, movement: &Movement) {
        let Some(view) = &self.view else { return };
        if view.contains_key(&id) && self.crowd.checks.is_open() {
            Checks::count(&self.crowd.checks.double_appears);
        }
        self.exact(id, movement);
        if let Some(view) = &mut self.view {
            view.insert(id, Seen { pos: movement.pos });
        }
    }

    fn moved(&mut self, id: u32, movement: &Movement, now: u32) {
        let Some(view) = &self.view else { return };
        let checks = &self.crowd.checks;
        match view.get(&id) {
            None if checks.is_open() => Checks::count(&checks.unknown_moves),
            Some(prev) if checks.is_open() => {
                if let (Some(me), Some(them)) = (
                    self.crowd.track(self.me),
                    self.crowd.settled(id, now).filter(|t| t.lie.is_none()),
                ) {
                    let truth = them.xy(now);
                    let t = tier(dist(me.xy(now), truth));
                    checks.stale[t].judge(dist(truth, ground(prev.pos)), bound_yd(t));
                }
            }
            _ => {}
        }
        self.exact(id, movement);
        if let Some(view) = &mut self.view {
            view.insert(id, Seen { pos: movement.pos });
        }
    }

    /// Checks who this bot is shown against who is really near, and where.
    fn sweep(&self, now: u32) {
        let (Some(view), Some(me)) = (&self.view, self.crowd.track(self.me)) else {
            return;
        };
        if now < self.welcomed_at + SETTLE_MS {
            return;
        }
        let checks = &self.crowd.checks;
        let here = me.xy(now);
        for id in 0..self.crowd.tracks.len() as u32 {
            let Some(them) = self.crowd.settled(id, now) else {
                continue;
            };
            if id == self.me || them.lie.is_some() {
                continue;
            }
            let truth = them.xy(now);
            let d = dist(here, truth);
            match view.get(&id) {
                None if d <= VIEW_YD - PRESENCE_SLACK_YD => checks.missing(d),
                Some(seen) => {
                    if d > VIEW_YD + PRESENCE_SLACK_YD {
                        Checks::count(&checks.spurious);
                    }
                    let t = tier(d);
                    checks.swept[t].judge(dist(truth, ground(seen.pos)), bound_yd(t));
                }
                None => {}
            }
        }
    }
}
