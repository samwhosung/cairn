use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use protocol::{ClientMessage, Frames, VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::runtime::Handle;
use tokio::sync::{Notify, mpsc};

use crate::world::{Input, Stamped};

const READ_BUF: usize = 16 << 10;
const UNREPORTED: u32 = u32::MAX;
const AFTER_EVERY_INPUT: u32 = u32::MAX;

pub struct Outbox {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued_bytes: Arc<AtomicUsize>,
    behind: Arc<AtomicU32>,
    hung_up: Arc<Notify>,
}

impl Drop for Outbox {
    fn drop(&mut self) {
        self.hung_up.notify_one();
    }
}

impl Outbox {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let behind = Arc::new(AtomicU32::new(UNREPORTED));
        let outbox = Self {
            tx,
            queued_bytes,
            behind,
            hung_up: Arc::new(Notify::new()),
        };
        (outbox, rx)
    }

    pub fn hung_up(&self) -> impl Future<Output = ()> + Send + use<> {
        let hung_up = self.hung_up.clone();
        async move { hung_up.notified().await }
    }

    pub fn behind_by(&self) -> impl Fn(u32) + Send + Sync + use<> {
        let behind = self.behind.clone();
        move |ticks| behind.store(ticks, Ordering::Relaxed)
    }

    pub fn behind_ticks(&self) -> Option<u32> {
        match self.behind.load(Ordering::Relaxed) {
            UNREPORTED => None,
            ticks => Some(ticks),
        }
    }

    pub fn send(&self, bytes: Vec<u8>) {
        self.queued_bytes.fetch_add(bytes.len(), Ordering::Relaxed);
        if let Err(e) = self.tx.send(bytes) {
            self.queued_bytes.fetch_sub(e.0.len(), Ordering::Relaxed);
        }
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes.load(Ordering::Relaxed)
    }

    pub fn on_written(&self) -> impl Fn(usize) + Send + Sync + use<> {
        let queued = self.queued_bytes.clone();
        move |n| {
            queued.fetch_sub(n, Ordering::Relaxed);
        }
    }
}

enum Admitting {
    Open(HashMap<u32, Outbox>),
    Stopped,
}

pub struct Shared {
    inbox: Mutex<Vec<Stamped>>,
    admitting: Mutex<Admitting>,
    next_conn: AtomicU32,
    pub latest_tick: AtomicU32,
    pub bytes_in: AtomicU64,
    pub stop: AtomicBool,
    started: Instant,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self {
            inbox: Mutex::new(Vec::new()),
            admitting: Mutex::new(Admitting::Open(HashMap::new())),
            next_conn: AtomicU32::new(0),
            latest_tick: AtomicU32::new(0),
            bytes_in: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            started: Instant::now(),
        }
    }

    pub fn ms_since_start(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    pub fn take_inputs(&self) -> Vec<Stamped> {
        let mut inputs =
            std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        inputs.sort_unstable_by_key(|s| (s.conn, s.nth));
        inputs
    }

    pub fn hold_outbox(&self, conn: u32, outbox: Outbox) {
        match &mut *self.admitting() {
            Admitting::Open(held) => {
                held.insert(conn, outbox);
            }
            Admitting::Stopped => drop(outbox),
        }
    }

    pub fn take_outbox(&self, conn: u32) -> Option<Outbox> {
        match &mut *self.admitting() {
            Admitting::Open(held) => held.remove(&conn),
            Admitting::Stopped => None,
        }
    }

    /// Closes every connection not yet admitted, and every one that comes after.
    pub fn stop_admitting(&self) {
        *self.admitting() = Admitting::Stopped;
    }

    fn admitting(&self) -> MutexGuard<'_, Admitting> {
        self.admitting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn leave_next_tick(&self, conn: u32) {
        let mut leave = vec![Stamped {
            conn,
            nth: AFTER_EVERY_INPUT,
            received_ms: self.ms_since_start(),
            input: Input::Leave,
        }];
        self.push(&mut leave);
    }

    fn push(&self, batch: &mut Vec<Stamped>) {
        if !batch.is_empty() {
            let mut inbox = self.inbox.lock().unwrap_or_else(PoisonError::into_inner);
            inbox.append(batch);
        }
    }
}

pub async fn accept(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            tokio::task::yield_now().await;
            continue;
        };
        let _ = socket.set_nodelay(true);
        let (reader, writer) = socket.into_split();
        let incoming = Incoming::Socket(reader, vec![0; READ_BUF]);
        open(
            &shared,
            &Handle::current(),
            Standing::Guest,
            incoming,
            |rx, outbox| write(writer, rx, outbox.on_written()),
        );
    }
}

/// A player's connection from inside the server's own process: the frames a socket would carry,
/// over channels.
pub struct InProcess {
    to_server: mpsc::UnboundedSender<Vec<u8>>,
    from_server: std_mpsc::Receiver<(Vec<u8>, Instant)>,
}

impl InProcess {
    /// Hands the server bytes as a socket would carry them; `BrokenPipe` once the server has let
    /// the connection go.
    pub fn send(&self, bytes: Vec<u8>) -> io::Result<()> {
        self.to_server
            .send(bytes)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    /// The next frame the server wrote, with when it wrote it; `Disconnected` once the server has
    /// let the connection go.
    pub fn try_recv(&self) -> Result<(Vec<u8>, Instant), std_mpsc::TryRecvError> {
        self.from_server.try_recv()
    }
}

pub fn connect_host(shared: &Arc<Shared>, runtime: &Handle) -> InProcess {
    let (to_server, from_client) = mpsc::unbounded_channel();
    let (to_client, from_server) = std_mpsc::channel();
    let incoming = Incoming::Here(from_client, Vec::new());
    open(shared, runtime, Standing::Host, incoming, |rx, outbox| {
        hand_over(rx, to_client, outbox.on_written())
    });
    InProcess {
        to_server,
        from_server,
    }
}

#[derive(Clone, Copy)]
enum Standing {
    Guest,
    Host,
}

enum Incoming {
    Socket(OwnedReadHalf, Vec<u8>),
    Here(mpsc::UnboundedReceiver<Vec<u8>>, Vec<u8>),
}

impl Incoming {
    async fn next(&mut self) -> Option<&[u8]> {
        match self {
            Self::Socket(r, buf) => match r.read(buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => Some(&buf[..n]),
            },
            Self::Here(rx, last) => {
                *last = rx.recv().await?;
                Some(last)
            }
        }
    }
}

fn open<W: Future<Output = ()> + Send + 'static>(
    shared: &Arc<Shared>,
    runtime: &Handle,
    standing: Standing,
    incoming: Incoming,
    writing: impl FnOnce(mpsc::UnboundedReceiver<Vec<u8>>, &Outbox) -> W,
) {
    let conn = shared.next_conn.fetch_add(1, Ordering::Relaxed);
    let (outbox, rx) = Outbox::channel();
    let (behind, hung_up) = (outbox.behind.clone(), outbox.hung_up());
    let writer = runtime.spawn(writing(rx, &outbox));
    shared.hold_outbox(conn, outbox);
    let reader = runtime.spawn(read(conn, incoming, standing, behind, shared.clone()));
    runtime.spawn(async move {
        hung_up.await;
        reader.abort();
        writer.abort();
    });
}

async fn hand_over(
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    to: std_mpsc::Sender<(Vec<u8>, Instant)>,
    on_written: impl Fn(usize),
) {
    while let Some(bytes) = rx.recv().await {
        let n = bytes.len();
        if to.send((bytes, Instant::now())).is_err() {
            break;
        }
        on_written(n);
    }
}

async fn write(
    mut w: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    on_written: impl Fn(usize),
) {
    while let Some(bytes) = rx.recv().await {
        if w.write_all(&bytes).await.is_err() {
            break;
        }
        on_written(bytes.len());
    }
}

async fn read(
    conn: u32,
    mut incoming: Incoming,
    standing: Standing,
    behind: Arc<AtomicU32>,
    shared: Arc<Shared>,
) {
    let mut frames = Frames::default();
    let (mut nth, mut joined, mut batch) = (0u32, false, Vec::new());
    'conn: while let Some(bytes) = incoming.next().await {
        shared
            .bytes_in
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        frames.extend(bytes);
        let received_ms = shared.ms_since_start();
        loop {
            let input = match frames.next_frame().map(|f| f.map(ClientMessage::read)) {
                Ok(None) => break,
                Ok(Some(Ok(ClientMessage::Hello(h)))) if !joined && h.version == VERSION => {
                    joined = true;
                    match standing {
                        Standing::Guest => Input::Join(h),
                        Standing::Host => Input::HostJoin(h),
                    }
                }
                Ok(Some(Ok(ClientMessage::Claim(c)))) if joined => Input::Claim(c),
                Ok(Some(Ok(ClientMessage::Teleport(c)))) if joined => Input::Teleport(c),
                Ok(Some(Ok(ClientMessage::Action(number)))) if joined => Input::Action(number),
                Ok(Some(Ok(ClientMessage::Seen(tick)))) if joined => {
                    let now = shared.latest_tick.load(Ordering::Relaxed);
                    behind.store(now.saturating_sub(tick), Ordering::Relaxed);
                    continue;
                }
                _ => {
                    shared.push(&mut batch);
                    break 'conn;
                }
            };
            batch.push(Stamped {
                conn,
                nth,
                received_ms,
                input,
            });
            nth += 1;
        }
        shared.push(&mut batch);
    }
    if joined {
        let received_ms = shared.ms_since_start();
        batch.push(Stamped {
            conn,
            nth,
            received_ms,
            input: Input::Leave,
        });
        shared.push(&mut batch);
    }
    drop(shared.take_outbox(conn));
}
