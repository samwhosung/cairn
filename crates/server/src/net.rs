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

    pub fn hold(&self, bytes: Vec<u8>) -> Held {
        self.queued_bytes.fetch_add(bytes.len(), Ordering::Relaxed);
        Held {
            tx: self.tx.clone(),
            bytes,
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

pub struct Held {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    bytes: Vec<u8>,
}

impl Held {
    pub fn send(self) {
        let _ = self.tx.send(self.bytes);
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

    pub fn next_conn(&self) -> u32 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
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

    pub fn push(&self, batch: &mut Vec<Stamped>) {
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

pub struct ServerEnd {
    pub from_client: mpsc::UnboundedReceiver<Vec<u8>>,
    pub to_client: std_mpsc::Sender<(Vec<u8>, Instant)>,
}

impl InProcess {
    pub(crate) fn open() -> (Self, ServerEnd) {
        let (to_server, from_client) = mpsc::unbounded_channel();
        let (to_client, from_server) = std_mpsc::channel();
        let client = Self {
            to_server,
            from_server,
        };
        let end = ServerEnd {
            from_client,
            to_client,
        };
        (client, end)
    }
}

pub fn connect_host(shared: &Arc<Shared>, runtime: &Handle) -> InProcess {
    let (client, end) = InProcess::open();
    let incoming = Incoming::Here(end.from_client, Vec::new());
    open(shared, runtime, Standing::Host, incoming, |rx, outbox| {
        hand_over(rx, end.to_client, outbox.on_written())
    });
    client
}

#[derive(Clone, Copy)]
pub enum Standing {
    Guest,
    /// May teleport.
    Host,
}

/// Bytes that break the protocol, for which the connection is closed.
pub struct Broken;

pub struct Reader {
    pub conn: u32,
    standing: Standing,
    frames: Frames,
    nth: u32,
    joined: bool,
}

impl Reader {
    pub fn new(conn: u32, standing: Standing) -> Self {
        Self {
            conn,
            standing,
            frames: Frames::default(),
            nth: 0,
            joined: false,
        }
    }

    /// Reads `bytes`, received at `received_ms` when the latest tick was `latest_tick`, into
    /// `inputs`.
    pub fn read(
        &mut self,
        bytes: &[u8],
        received_ms: u32,
        latest_tick: u32,
        behind_by: impl Fn(u32),
        inputs: &mut Vec<Stamped>,
    ) -> Result<(), Broken> {
        self.frames.extend(bytes);
        loop {
            let input = match self.frames.next_frame().map(|f| f.map(ClientMessage::read)) {
                Ok(None) => return Ok(()),
                Ok(Some(Ok(ClientMessage::Hello(h)))) if !self.joined && h.version == VERSION => {
                    self.joined = true;
                    match self.standing {
                        Standing::Guest => Input::Join(h),
                        Standing::Host => Input::HostJoin(h),
                    }
                }
                Ok(Some(Ok(ClientMessage::Claim(c)))) if self.joined => Input::Claim(c),
                Ok(Some(Ok(ClientMessage::Teleport(c)))) if self.joined => Input::Teleport(c),
                Ok(Some(Ok(ClientMessage::Action(number)))) if self.joined => Input::Action(number),
                Ok(Some(Ok(ClientMessage::Seen(tick)))) if self.joined => {
                    behind_by(latest_tick.saturating_sub(tick));
                    continue;
                }
                _ => return Err(Broken),
            };
            inputs.push(Stamped {
                conn: self.conn,
                nth: self.nth,
                received_ms,
                input,
            });
            self.nth += 1;
        }
    }

    /// The leave of a connection that has closed, once its hello was in.
    pub fn leave(&self, received_ms: u32) -> Option<Stamped> {
        self.joined.then_some(Stamped {
            conn: self.conn,
            nth: self.nth,
            received_ms,
            input: Input::Leave,
        })
    }
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
    let conn = shared.next_conn();
    let (outbox, rx) = Outbox::channel();
    let (behind, hung_up) = (outbox.behind.clone(), outbox.hung_up());
    let writer = runtime.spawn(writing(rx, &outbox));
    shared.hold_outbox(conn, outbox);
    let reader = Reader::new(conn, standing);
    let reader = runtime.spawn(read(reader, incoming, behind, shared.clone()));
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
    mut reader: Reader,
    mut incoming: Incoming,
    behind: Arc<AtomicU32>,
    shared: Arc<Shared>,
) {
    let mut batch = Vec::new();
    while let Some(bytes) = incoming.next().await {
        shared
            .bytes_in
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let (received_ms, latest) = (
            shared.ms_since_start(),
            shared.latest_tick.load(Ordering::Relaxed),
        );
        let behind_by = |ticks| behind.store(ticks, Ordering::Relaxed);
        let read = reader.read(bytes, received_ms, latest, behind_by, &mut batch);
        shared.push(&mut batch);
        if read.is_err() {
            break;
        }
    }
    batch.extend(reader.leave(shared.ms_since_start()));
    shared.push(&mut batch);
    drop(shared.take_outbox(reader.conn));
}
