use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use protocol::{ClientMessage, Frames, VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::world::{Input, Stamped};

const READ_BUF: usize = 16 << 10;
const UNREPORTED: u32 = u32::MAX;

pub struct Outbox {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued_bytes: Arc<AtomicUsize>,
    behind: Arc<AtomicU32>,
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
        };
        (outbox, rx)
    }

    #[cfg(test)]
    pub fn behind_by(&self) -> impl Fn(u32) + use<> {
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

    pub fn on_written(&self) -> impl Fn(usize) + Send + use<> {
        let queued = self.queued_bytes.clone();
        move |n| {
            queued.fetch_sub(n, Ordering::Relaxed);
        }
    }
}

pub struct Shared {
    inbox: Mutex<Vec<Stamped>>,
    unadmitted: Mutex<HashMap<u32, Outbox>>,
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
            unadmitted: Mutex::new(HashMap::new()),
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
        self.unadmitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(conn, outbox);
    }

    pub fn take_outbox(&self, conn: u32) -> Option<Outbox> {
        self.unadmitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&conn)
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
        let conn = shared.next_conn.fetch_add(1, Ordering::Relaxed);
        let (reader, writer) = socket.into_split();
        let (outbox, rx) = Outbox::channel();
        let (on_written, behind) = (outbox.on_written(), outbox.behind.clone());
        shared.hold_outbox(conn, outbox);
        tokio::spawn(write(writer, rx, on_written));
        tokio::spawn(read(conn, reader, behind, shared.clone()));
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

async fn read(conn: u32, mut r: OwnedReadHalf, behind: Arc<AtomicU32>, shared: Arc<Shared>) {
    let mut frames = Frames::default();
    let mut buf = vec![0u8; READ_BUF];
    let (mut nth, mut joined, mut batch) = (0u32, false, Vec::new());
    'conn: loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        shared.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
        frames.extend(&buf[..n]);
        let received_ms = shared.ms_since_start();
        loop {
            let input = match frames.next_frame().map(|f| f.map(ClientMessage::read)) {
                Ok(None) => break,
                Ok(Some(Ok(ClientMessage::Hello(h)))) if !joined && h.version == VERSION => {
                    joined = true;
                    Input::Join(h)
                }
                Ok(Some(Ok(ClientMessage::Claim(c)))) if joined => Input::Claim(c),
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
    shared
        .unadmitted
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&conn);
}
